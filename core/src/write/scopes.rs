//! "Who can open these passwords": every `.gpg-id` in the store, the keys
//! it names, and which entries under it are encrypted for some other key.
//!
//! Read-only, and nothing is decrypted: the keys come from the `.gpg-id`
//! lines and the keys pinned on this device ([`super::keyring`]); who an
//! entry is encrypted for comes from its clear-text recipient packets
//! ([`crate::recipients`]). An entry is "for another key" when a packet
//! names a key ID that none of its folder's keys has. That is certain only
//! when paw knows every key of the folder: a line paw has no key for may
//! name a certificate whose encryption subkey is the one in the packet, so
//! such entries are counted as `unsure`, not listed. So are entries with a
//! hidden recipient (key ID zero) or a passphrase packet.
//!
//! The screen shows what it can read. A `.gpg-id` it cannot read (a link,
//! a file the disk refuses) is left out, and so are the entries it would
//! govern, rather than hiding every other folder behind one error: saving
//! under it says what is wrong. One that is not UTF-8 is shown as far as it
//! can be read, as the sync report shows it.
//!
//! Entry names are returned for the screen only. Nothing here logs them.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::WriteError;
use super::keyring::{Keyring, LineStatus, line_fingerprint, line_key_id};
use crate::fingerprint::Fingerprint;
use crate::path::folder_of_gpg_id;
use crate::recipients::{Recipient, entry_recipients};
use crate::path::GPG_ID;
use crate::store::gpg_id::{governing_gpg_id, shown_gpg_id_lines};
use crate::store::read::{read_entry, read_file};
use crate::store::tree::StoreTree;
use crate::lock::Shared;

/// One key a `.gpg-id` names, as the screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ScopeKey {
    /// The line as written in `.gpg-id`.
    pub line: String,
    /// The last 16 hex digits of the line, if it is a fingerprint or key ID;
    /// else of the pinned key it resolves to; else `None` (an e-mail
    /// address or a name paw has no key for).
    pub short_id: Option<String>,
    /// The line resolves to the key on the plugged-in card that this store
    /// was set up with.
    pub this_key: bool,
    pub status: LineStatus,
}

/// One `.gpg-id` and the entries it governs.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RecipientScope {
    /// The folder it is in: empty for the top of the store.
    pub folder: String,
    /// Its path in the store, e.g. `.gpg-id` or `work/.gpg-id`.
    pub gpg_id: String,
    pub keys: Vec<ScopeKey>,
    pub entry_count: u32,
    /// Entries it governs that are encrypted for a key it does not name.
    /// For the screen only; never log these.
    pub other_key_entries: Vec<String>,
    /// Entries that may be for another key: they name a key paw cannot
    /// place (a line of this `.gpg-id` has no key on this device), a hidden
    /// recipient, or a passphrase.
    pub unsure_entries: u32,
}

/// The keys a `.gpg-id` names: by fingerprint (its own lines', and its
/// pinned keys'), and by key ID alone for a line no pinned key is behind.
#[derive(Default)]
struct Named {
    fingerprints: Vec<Fingerprint>,
    key_ids: HashSet<String>,
}

impl Named {
    /// Whether `recipient` is one of these keys: [`Recipient::names`] for a
    /// fingerprint, the key ID when that is all the line gives (also of a
    /// packet that names its key by fingerprint, as v6 packets do).
    fn has(&self, recipient: &Recipient) -> bool {
        let by_key_id = match recipient {
            Recipient::KeyId { id } => Some(id.clone()),
            Recipient::Fingerprint { fingerprint } => Fingerprint::parse(fingerprint).map(|f| f.key_id().to_owned()),
            Recipient::Hidden => None,
        };
        self.fingerprints.iter().any(|fingerprint| recipient.names(fingerprint))
            || by_key_id.is_some_and(|id| self.key_ids.contains(&id))
    }
}

/// Every `.gpg-id` of the keyring's store, top first, with what it says.
pub fn recipient_scopes(_reading: &Shared, keyring: &Keyring) -> Result<Vec<RecipientScope>, WriteError> {
    let root = keyring.repo();
    let tree = StoreTree::scan(root)?;
    let mut governed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut by_folder: BTreeMap<String, String> = BTreeMap::new();
    for entry in tree.entries() {
        let gpg_id = match by_folder.get(&entry.folder) {
            Some(found) => found.clone(),
            None => {
                let Ok((found, _)) = governing_gpg_id(root, &entry.folder) else { continue };
                by_folder.insert(entry.folder.clone(), found.clone());
                found
            }
        };
        governed.entry(gpg_id).or_default().push(entry.path.clone());
    }
    // `.gpg-id` files that govern no entry still say who could open one.
    let mut files: BTreeSet<String> = governed.keys().cloned().collect();
    files.insert(GPG_ID.into());
    if let Ok(paths) = super::keyring::gpg_id_paths(root) {
        files.extend(paths);
    }

    let mut scopes = Vec::new();
    for gpg_id in files {
        // Not in the working copy, or not one paw can read (see above).
        let Ok(Some(bytes)) = read_file(root, &gpg_id) else { continue };
        let lines = shown_gpg_id_lines(&bytes);
        let mut named = Named::default();
        let mut complete = true;
        let mut keys = Vec::new();
        for line in lines {
            let status = keyring.line_status(&line);
            let pinned = match &status {
                LineStatus::Ready { fingerprint, .. } => keyring.keys().iter().find(|k| k.fingerprint.as_str() == fingerprint),
                _ => None,
            };
            match pinned {
                Some(key) => named.fingerprints.push(key.fingerprint.clone()),
                None => {
                    complete = false;
                    named.key_ids.extend(line_key_id(&line));
                }
            }
            named.fingerprints.extend(line_fingerprint(&line));
            let short_id = line_key_id(&line).or_else(|| pinned.map(|k| k.fingerprint.key_id().to_owned()));
            keys.push(ScopeKey { line, short_id, this_key: pinned.is_some(), status });
        }
        let entries = governed.remove(&gpg_id).unwrap_or_default();
        let mut other_key_entries = Vec::new();
        let mut unsure_entries = 0u32;
        for entry in &entries {
            // An unreadable file is not for another key; the entry's own
            // screen says what is wrong with it.
            let Ok(bytes) = read_entry(root, entry) else { continue };
            let Ok(found) = entry_recipients(&bytes) else { continue };
            let stranger = found.keys.iter().any(|recipient| *recipient != Recipient::Hidden && !named.has(recipient));
            // A hidden recipient may be any key, and a passphrase opens it
            // without one: neither is certain to be for another key.
            let maybe = found.passphrase || found.keys.contains(&Recipient::Hidden);
            if !stranger && !maybe {
                continue;
            }
            if stranger && complete {
                other_key_entries.push(entry.clone());
            } else {
                unsure_entries += 1;
            }
        }
        other_key_entries.sort();
        scopes.push(RecipientScope {
            folder: folder_of_gpg_id(&gpg_id).to_owned(),
            gpg_id,
            keys,
            entry_count: entries.len() as u32,
            other_key_entries,
            unsure_entries,
        });
    }
    scopes.sort_by(|a, b| (!a.folder.is_empty(), &a.folder).cmp(&(!b.folder.is_empty(), &b.folder)));
    Ok(scopes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_ids_come_from_fingerprints_and_long_ids() {
        assert_eq!(line_key_id("0x1122334455667788").as_deref(), Some("1122334455667788"));
        assert_eq!(
            line_key_id("AAAA BBBB CCCC DDDD EEEE  FFFF 0000 1111 2222 3333").as_deref(),
            Some("0000111122223333")
        );
        assert_eq!(line_key_id("you@example.com"), None);
        assert_eq!(line_key_id("ABCD1234"), None);
    }

    #[test]
    fn a_v6_packet_is_for_a_line_that_gives_only_its_key_id() {
        let fingerprint = format!("1122334455667788{}", "A".repeat(48));
        let named = Named { fingerprints: Vec::new(), key_ids: ["1122334455667788".to_owned()].into() };
        assert!(named.has(&Recipient::Fingerprint { fingerprint }));
        assert!(!named.has(&Recipient::Fingerprint { fingerprint: "B".repeat(64) }));
        assert!(!named.has(&Recipient::Hidden));
    }
}
