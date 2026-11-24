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
//! such entries are counted as `unsure`, not listed.
//!
//! Entry names are returned for the screen only. Nothing here logs them.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use super::WriteError;
use super::keyring::{KeySource, Keyring, LineStatus};
use crate::recipients::{Recipient, entry_recipients};
use crate::store::gpg_id::{governing_gpg_id, parse_gpg_id};
use crate::store::read::read_entry;
use crate::store::tree::StoreTree;

/// One key a `.gpg-id` names, as the screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ScopeKey {
    /// The line as written in `.gpg-id`.
    pub line: String,
    /// The last 16 hex digits of the line, if it is a fingerprint or key ID;
    /// else of the pinned key it resolves to; else `None` (an e-mail
    /// address or a name paw has no key for).
    pub short_id: Option<String>,
    /// E-mail addresses from the pinned certificate's user IDs, if any.
    pub emails: Vec<String>,
    /// The line resolves to the key on the plugged-in card that this store
    /// was set up with.
    pub this_key: bool,
    /// How paw would encrypt to it.
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
    /// Entries it governs.
    pub entry_count: u32,
    /// Entries it governs that are encrypted for a key it does not name.
    /// For the screen only; never log these.
    pub other_key_entries: Vec<String>,
    /// Entries naming a key paw cannot place, because a line of this
    /// `.gpg-id` has no key on this device.
    pub unsure_entries: u32,
}

fn compact(text: &str) -> String {
    let trimmed = text.trim();
    let bare = trimmed.strip_suffix('!').unwrap_or(trimmed);
    let bare = bare.strip_prefix("0x").or_else(|| bare.strip_prefix("0X")).unwrap_or(bare);
    bare.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase()
}

/// The 64-bit key ID a recipient packet would name for a key with this
/// fingerprint or long ID (16, 40 or 64 hex digits).
fn key_id(text: &str) -> Option<String> {
    let hex = compact(text);
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        16 => Some(hex),
        40 => Some(hex[24..].to_owned()),
        64 => Some(hex[..16].to_owned()),
        _ => None,
    }
}

/// How a packet's recipient reads as a key ID.
fn packet_key_id(recipient: &Recipient) -> Option<String> {
    match recipient {
        Recipient::KeyId { id } => Some(id.clone()),
        Recipient::Fingerprint { fingerprint } => key_id(fingerprint),
        Recipient::Hidden => None,
    }
}

/// The folder a `.gpg-id` path is in.
fn folder_of(gpg_id: &str) -> String {
    gpg_id.strip_suffix(".gpg-id").unwrap_or(gpg_id).trim_end_matches('/').to_owned()
}

/// Every `.gpg-id` of the store at `root`, top first, with what it says.
pub fn recipient_scopes(keyring: &Keyring, root: &Path, now: u32) -> Result<Vec<RecipientScope>, WriteError> {
    let tree = StoreTree::scan(root)?;
    // The governing file of each folder that holds entries, and the entries.
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
    if root.join(".gpg-id").is_file() {
        files.insert(".gpg-id".into());
    }
    if let Ok(paths) = super::keyring::gpg_id_paths(root) {
        files.extend(paths.into_iter().filter(|p| root.join(p).is_file()));
    }

    let mut scopes = Vec::new();
    for gpg_id in files {
        let Ok(bytes) = std::fs::read(root.join(&gpg_id)) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let lines = parse_gpg_id(&text);
        let mut known: HashSet<String> = HashSet::new();
        let mut complete = true;
        let mut keys = Vec::new();
        for line in lines {
            let status = keyring.line_status(&line, now);
            let pinned = match &status {
                LineStatus::Ready { fingerprint, .. } => keyring.keys().iter().find(|k| k.fingerprint == *fingerprint),
                _ => None,
            };
            if let Some(id) = key_id(&line) {
                known.insert(id);
            }
            match pinned {
                Some(key) => {
                    known.extend(key_id(&key.fingerprint));
                    known.extend(key.fingerprints.iter().filter_map(|f| key_id(f)));
                    known.extend(key.primary.iter().filter_map(|f| key_id(f)));
                }
                None => complete = false,
            }
            let short_id = key_id(&line).or_else(|| pinned.and_then(|k| key_id(&k.fingerprint)));
            let this_key = matches!(status, LineStatus::Ready { source: KeySource::Card, .. });
            keys.push(ScopeKey {
                line,
                short_id,
                emails: pinned.map(|k| k.emails.clone()).unwrap_or_default(),
                this_key,
                status,
            });
        }
        let entries = governed.remove(&gpg_id).unwrap_or_default();
        let mut other_key_entries = Vec::new();
        let mut unsure_entries = 0u32;
        for entry in &entries {
            // An unreadable file is not for another key; the entry's own
            // screen says what is wrong with it.
            let Ok(bytes) = read_entry(root, entry) else { continue };
            let Ok(found) = entry_recipients(&bytes) else { continue };
            let stranger = found
                .keys
                .iter()
                .filter_map(packet_key_id)
                .any(|id| !known.contains(&id));
            if !stranger {
                continue;
            }
            if complete {
                other_key_entries.push(entry.clone());
            } else {
                unsure_entries += 1;
            }
        }
        other_key_entries.sort();
        scopes.push(RecipientScope {
            folder: folder_of(&gpg_id),
            gpg_id,
            keys,
            entry_count: entries.len() as u32,
            other_key_entries,
            unsure_entries,
        });
    }
    // Top of the store first, then by folder.
    scopes.sort_by(|a, b| (!a.folder.is_empty(), &a.folder).cmp(&(!b.folder.is_empty(), &b.folder)));
    Ok(scopes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_ids_come_from_fingerprints_and_long_ids() {
        assert_eq!(key_id("0x1122334455667788").as_deref(), Some("1122334455667788"));
        assert_eq!(
            key_id("AAAA BBBB CCCC DDDD EEEE  FFFF 0000 1111 2222 3333").as_deref(),
            Some("0000111122223333")
        );
        assert_eq!(key_id("you@example.com"), None);
        assert_eq!(key_id("ABCD1234"), None);
    }

    #[test]
    fn folders_of_gpg_ids() {
        assert_eq!(folder_of(".gpg-id"), "");
        assert_eq!(folder_of("work/.gpg-id"), "work");
        assert_eq!(folder_of("a/b/.gpg-id"), "a/b");
    }
}
