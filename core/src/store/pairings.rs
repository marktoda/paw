//! Sites a name-only entry has been filled on, kept on this device.
//!
//! An entry called `bankofamerica` names no domain, so it matches
//! `bankofamerica.com` and `bankofamerica.co` alike ([`super::service`]).
//! The first fill on a domain asks; once it has filled there, the pair
//! (entry path, registrable domain) is kept here and later fills on that
//! domain do not ask. The same entry on another domain of the same name is
//! then a lookalike.
//!
//! The file is `site-pairings.json` in the store's state directory, next to
//! the repository, never in it: it is not synced and removing the store
//! removes it. It holds names and domains only, nothing from inside an
//! entry.
//!
//! The app and the AutoFill extension both write it. There is no file lock
//! (iOS kills a suspended process that holds one in a shared container, see
//! [`crate::lock`]): each write goes to a temporary file that is renamed
//! over the old one, so a reader sees a whole file or none. Threads of one
//! process take turns; two processes writing in the same instant can lose
//! one pairing, which costs one more question, never a wrong fill. A file
//! that cannot be read as pairings is ignored and replaced on the next
//! write. Size is bounded: [`MAX_ENTRIES`] entries of [`MAX_SITES`] sites,
//! the least recently filled dropped first.

use std::collections::HashSet;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

use super::PassStore;
use super::service::{host_of, names_site, registrable, registrable_domain, site_name};
use crate::PawError;

/// The file, in the state directory.
pub const FILE: &str = "site-pairings.json";
/// The most entries kept.
pub const MAX_ENTRIES: usize = 300;
/// The most sites kept per entry.
pub const MAX_SITES: usize = 4;
/// A longer path is not kept.
const MAX_PATH_BYTES: usize = 512;
/// A larger file is not read.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const VERSION: u32 = 1;
const TEMP_PREFIX: &str = ".site-pairings.tmp-";
/// A temporary file this old belongs to a writer that died mid-write.
const STALE_TEMP: Duration = Duration::from_secs(60);

/// One entry and the registrable domains it has been filled on, least
/// recent first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct Pairing {
    pub entry: String,
    pub sites: Vec<String>,
}

/// One entry and one site, for a suggestion above the keyboard.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairedSite {
    pub entry: String,
    pub site: String,
}

#[derive(Serialize, Deserialize)]
struct Book {
    version: u32,
    pairings: Vec<Pairing>,
}

/// Threads of this process write one at a time.
static WRITING: Mutex<()> = Mutex::new(());

/// A site worth keeping: a lower-case registrable domain.
fn valid_site(site: &str) -> bool {
    site.len() <= 253 && host_of(site).as_deref() == Some(site) && registrable(site) == Some(site)
}

/// The pairings file of one store.
#[derive(Debug, Clone)]
pub struct PairingFile {
    state_dir: PathBuf,
}

impl PairingFile {
    pub fn new(state_dir: &Path) -> Self {
        Self { state_dir: state_dir.to_path_buf() }
    }

    pub fn path(&self) -> PathBuf {
        self.state_dir.join(FILE)
    }

    /// Every pairing, least recently filled first. A missing, unreadable
    /// or malformed file has none; malformed items are left out.
    pub fn load(&self) -> Vec<Pairing> {
        let Ok(file) = std::fs::File::open(self.path()) else { return Vec::new() };
        let mut bytes = Vec::new();
        if file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > MAX_FILE_BYTES {
            return Vec::new();
        }
        let Ok(book) = serde_json::from_slice::<Book>(&bytes) else { return Vec::new() };
        if book.version != VERSION {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        let mut pairings: Vec<Pairing> = book
            .pairings
            .into_iter()
            .filter(|p| !p.entry.is_empty() && p.entry.len() <= MAX_PATH_BYTES)
            .filter(|p| seen.insert(p.entry.clone()))
            .map(|mut p| {
                let mut sites = HashSet::new();
                p.sites.retain(|s| valid_site(s) && sites.insert(s.clone()));
                let extra = p.sites.len().saturating_sub(MAX_SITES);
                p.sites.drain(..extra);
                p
            })
            .filter(|p| !p.sites.is_empty())
            .collect();
        let extra = pairings.len().saturating_sub(MAX_ENTRIES);
        pairings.drain(..extra);
        pairings
    }

    /// Reads, changes and writes the file. `change` says whether it changed
    /// anything; nothing is written if not. Refuses if the state directory
    /// is gone (the store was removed): it is never made again here.
    fn update(&self, change: impl FnOnce(&mut Vec<Pairing>) -> bool) -> std::io::Result<()> {
        let _turn = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        if !self.state_dir.is_dir() {
            return Err(std::io::Error::new(ErrorKind::NotFound, "the store's folder is gone"));
        }
        let mut pairings = self.load();
        if !change(&mut pairings) {
            return Ok(());
        }
        let extra = pairings.len().saturating_sub(MAX_ENTRIES);
        pairings.drain(..extra);
        let bytes = serde_json::to_vec(&Book { version: VERSION, pairings })
            .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
        self.write_atomic(&bytes)
    }

    fn write_atomic(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.remove_stale_temps();
        let mut tag = [0u8; 12];
        OsRng.fill_bytes(&mut tag);
        let tag: String = tag.iter().map(|b| format!("{b:02x}")).collect();
        let temp = self.state_dir.join(format!("{TEMP_PREFIX}{tag}"));
        let written = (|| {
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            std::fs::rename(&temp, self.path())
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        written
    }

    fn remove_stale_temps(&self) {
        let Ok(listing) = std::fs::read_dir(&self.state_dir) else { return };
        for child in listing.flatten() {
            if !child.file_name().to_str().is_some_and(|n| n.starts_with(TEMP_PREFIX)) {
                continue;
            }
            let old = child
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|at| SystemTime::now().duration_since(at).ok())
                .is_some_and(|age| age >= STALE_TEMP);
            if old {
                let _ = std::fs::remove_file(child.path());
            }
        }
    }

    /// Keeps that `entry` was filled on `site` (a registrable domain), as
    /// the most recent pairing. Entries not in `existing` are dropped
    /// while at it, if given.
    pub fn record(&self, entry: &str, site: &str, existing: Option<&HashSet<&str>>) -> std::io::Result<()> {
        if entry.is_empty() || entry.len() > MAX_PATH_BYTES || !valid_site(site) {
            return Err(std::io::Error::new(ErrorKind::InvalidInput, "not a pairing paw keeps"));
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

    /// Forgets one entry's pairings, or with `None` every pairing.
    pub fn clear(&self, entry: Option<&str>) -> std::io::Result<()> {
        match entry {
            Some(entry) => self.update(|pairings| {
                let before = pairings.len();
                pairings.retain(|p| p.entry != entry);
                pairings.len() != before
            }),
            None => {
                let _turn = WRITING.lock().unwrap_or_else(|e| e.into_inner());
                match std::fs::remove_file(self.path()) {
                    Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                }
            }
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

impl SitePairings {
    pub fn file(&self) -> &PairingFile {
        &self.file
    }
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
    /// picked from search for another site is never paired.
    pub fn record_pairing(&self, store: Arc<PassStore>, entry: String, service: String) -> Result<bool, PawError> {
        let Some(host) = host_of(&service) else { return Ok(false) };
        let (Some(name), Some(site)) = (site_name(&host), registrable_domain(&service)) else {
            return Ok(false);
        };
        let entries = store.entries();
        if !entries.iter().any(|e| e.path == entry) || !names_site(&entry, &name) {
            return Ok(false);
        }
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
