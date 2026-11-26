//! Sites a name-only entry has been filled on, kept on this device.
//!
//! An entry called `bankofamerica` names no domain, so it matches
//! `bankofamerica.com` and `bankofamerica.co` alike ([`super::service`]).
//! The first fill on a domain asks; once it has filled there, the pair
//! (entry path, registrable domain) is kept here and later fills on that
//! domain do not ask. The same entry on another domain of the same name is
//! then a lookalike.
//!
//! The file is `site-pairings.json` in the store's state directory
//! ([`crate::files::list`]). It holds names and domains only, nothing from
//! inside an entry. The app and the AutoFill extension both write it; two
//! processes writing in the same instant can lose one pairing, which costs
//! one more question, never a wrong fill. Size is bounded: [`MAX_ENTRIES`]
//! entries of [`MAX_SITES`] sites, the least recently filled dropped first.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::PassStore;
use super::service::{MatchKind, NamePairing, host_of, match_site, names_site, registrable, site_name};
use crate::files::list::{ListFile, ListItem, ListKind, MAX_PATH_BYTES};
use crate::PawError;

/// The most entries kept.
pub const MAX_ENTRIES: usize = 300;
/// The most sites kept per entry.
pub const MAX_SITES: usize = 4;

static LIST: ListKind = ListKind {
    file: crate::files::layout::SITE_PAIRINGS,
    key: "pairings",
    temp_prefix: ".site-pairings.tmp-",
    max_items: MAX_ENTRIES,
    on_marker: None,
    writing: Mutex::new(()),
};

/// One entry and the registrable domains it has been filled on, least
/// recent first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct Pairing {
    pub entry: String,
    pub sites: Vec<String>,
}

impl ListItem for Pairing {
    fn entry(&self) -> &str {
        &self.entry
    }

    fn clean(mut self) -> Option<Self> {
        let mut sites = HashSet::new();
        self.sites.retain(|s| valid_site(s) && sites.insert(s.clone()));
        let extra = self.sites.len().saturating_sub(MAX_SITES);
        self.sites.drain(..extra);
        (!self.sites.is_empty()).then_some(self)
    }
}

/// One entry and one site, for a suggestion above the keyboard.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairedSite {
    pub entry: String,
    pub site: String,
}

/// A site worth keeping: a lower-case registrable domain.
fn valid_site(site: &str) -> bool {
    site.len() <= 253 && host_of(site).as_deref() == Some(site) && registrable(site) == Some(site)
}

/// The pairings file of one store.
pub struct PairingFile {
    list: ListFile,
}

impl PairingFile {
    pub fn new(state_dir: &Path) -> Self {
        Self { list: ListFile::new(state_dir, &LIST) }
    }

    pub fn path(&self) -> PathBuf {
        self.list.path()
    }

    /// Every pairing, least recently filled first. A missing, unreadable
    /// or malformed file has none; malformed items are left out.
    pub fn load(&self) -> Vec<Pairing> {
        self.list.load()
    }

    /// Reads, changes and writes the file. `change` says whether it changed
    /// anything; nothing is written if not. Refuses if the state directory
    /// is gone (the store was removed): it is never made again here.
    fn update(&self, change: impl FnOnce(&mut Vec<Pairing>) -> bool) -> std::io::Result<()> {
        self.list.update(change).map(|_| ())
    }

/// Keeps that `entry` was filled on `site` (a registrable domain), as
    /// the most recent pairing. Entries not in `existing` are dropped
    /// while at it, if given.
    pub fn record(&self, entry: &str, site: &str, existing: Option<&HashSet<&str>>) -> std::io::Result<()> {
        if entry.is_empty() || entry.len() > MAX_PATH_BYTES || !valid_site(site) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a pairing paw keeps"));
        }
        self.update(|pairings| {
            if let Some(existing) = existing {
                pairings.retain(|p| existing.contains(p.entry.as_str()));
            }
            let mut sites = match pairings.iter().position(|p| p.entry == entry) {
                Some(at) => pairings.remove(at).sites,
                None => Vec::new(),
            };
            sites.retain(|s| s != site);
            sites.push(site.to_owned());
            let extra = sites.len().saturating_sub(MAX_SITES);
            sites.drain(..extra);
            pairings.push(Pairing { entry: entry.to_owned(), sites });
            true
        })
    }

    /// The entry moved from `from` to `to`: its pairings go with it.
    pub fn moved(&self, from: &str, to: &str) -> std::io::Result<()> {
        if from == to {
            return Ok(());
        }
        self.update(|pairings| {
            let Some(at) = pairings.iter().position(|p| p.entry == from) else { return false };
            let mut moving = pairings.remove(at);
            pairings.retain(|p| p.entry != to);
            if to.len() <= MAX_PATH_BYTES {
                moving.entry = to.to_owned();
                pairings.insert(at.min(pairings.len()), moving);
            }
            true
        })
    }

    /// Forgets that `entry` filled on `site`, and the entry altogether if
    /// that was its last site.
    pub fn forget_site(&self, entry: &str, site: &str) -> std::io::Result<()> {
        self.update(|pairings| {
            let mut changed = false;
            for pairing in pairings.iter_mut().filter(|p| p.entry == entry) {
                let before = pairing.sites.len();
                pairing.sites.retain(|s| s != site);
                changed |= pairing.sites.len() != before;
            }
            pairings.retain(|p| !p.sites.is_empty());
            changed
        })
    }

    /// Forgets one entry's pairings, or with `None` every pairing.
    pub fn clear(&self, entry: Option<&str>) -> std::io::Result<()> {
        match entry {
            Some(entry) => self.update(|pairings| {
                let before = pairings.len();
                pairings.retain(|p| p.entry != entry);
                pairings.len() != before
            }),
            None => self.list.clear(),
        }
    }
}

fn failed(e: std::io::Error) -> PawError {
    PawError::Store(format!("could not keep the site pairings: {e}"))
}

/// The pairings of one store, for the app and the extension.
#[derive(uniffi::Object)]
pub struct SitePairings {
    file: PairingFile,
}

#[uniffi::export]
impl SitePairings {
    /// The pairings of the store whose state directory is `state_dir`.
    /// Reads nothing until asked.
    #[uniffi::constructor]
    pub fn open(state_dir: String) -> Arc<Self> {
        Arc::new(Self { file: PairingFile::new(Path::new(&state_dir)) })
    }

    /// Keeps that `entry` has just filled on `service`. Call only after a
    /// fill the person confirmed. Refused (false, nothing kept) unless
    /// `entry` is in `store` and matches `service` by name: a password
    /// picked from search for another site is never paired. Refused too
    /// when the entry is already paired with another domain of the same
    /// name: `bankofamerica.co` after `bankofamerica.com` is a lookalike,
    /// and a fill there never makes it trusted. Clearing the entry's
    /// pairings is the only way to trust a second domain of one name.
    pub fn record_pairing(&self, store: Arc<PassStore>, entry: String, service: String) -> Result<bool, PawError> {
        let entries = store.entries();
        let Some(found) = entries.iter().find(|e| e.path == entry) else { return Ok(false) };
        let matched = match_site(std::slice::from_ref(found), &service, &self.file.load()).into_iter().next();
        let Some(site) = matched.and_then(|m| match m.kind {
            MatchKind::Name { pairing: NamePairing::Paired | NamePairing::Unpaired } => Some(m.site),
            _ => None,
        }) else {
            return Ok(false);
        };
        let existing: HashSet<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        self.file.record(&entry, &site, Some(&existing)).map_err(failed)?;
        Ok(true)
    }

    /// Every pairing, least recently filled first.
    pub fn pairings(&self) -> Vec<Pairing> {
        self.file.load()
    }

    /// Forgets the pairings of `entry`, or with `None` all of them.
    pub fn clear_pairings(&self, entry: Option<String>) -> Result<(), PawError> {
        self.file.clear(entry.as_deref()).map_err(failed)
    }

    /// Settings › AutoFill › Sites used: forgets that `entry` filled on
    /// `site`. The next fill there asks again.
    pub fn forget_site(&self, entry: String, site: String) -> Result<(), PawError> {
        self.file.forget_site(&entry, &site).map_err(failed)
    }

    /// The pairings that still hold in `store`: the entry is there and
    /// still matches the site by name. For the suggestions above the
    /// keyboard, one per entry and site, sorted by entry.
    pub fn paired_sites(&self, store: Arc<PassStore>) -> Vec<PairedSite> {
        let existing: HashSet<&str> = store.entries().iter().map(|e| e.path.as_str()).collect();
        let mut out: Vec<PairedSite> = self
            .file
            .load()
            .into_iter()
            .filter(|p| existing.contains(p.entry.as_str()))
            .flat_map(|p| {
                let entry = p.entry;
                p.sites.into_iter().filter_map(move |site| {
                    let name = site_name(&site)?;
                    names_site(&entry, &name).then(|| PairedSite { entry: entry.clone(), site })
                })
            })
            .collect();
        out.sort_by(|a, b| (&a.entry, &a.site).cmp(&(&b.entry, &b.site)));
        out
    }
}
