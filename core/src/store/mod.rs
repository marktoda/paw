//! Reading a password store that is checked out on this device.
//!
//! Everything here works from names and from the unencrypted start of each
//! file. Nothing in this module decrypts, and nothing needs the key or the
//! network. The directory is treated as hostile input throughout: see
//! [`read`] for how paw stays inside it.

pub mod codes;
pub mod gpg_id;
pub mod pairings;
pub mod read;
pub mod search;
pub mod service;
pub mod tree;

use std::path::Path;
use std::sync::Arc;

use crate::PawError;
use crate::fingerprint::Fingerprint;
use crate::lock::StoreDirs;
use crate::recipients::Access;
use search::SearchHit;
use pairings::SitePairings;
use service::MatchedEntry;
use tree::{EntryRef, FolderListing, StoreTree};

/// A password store on this device, as the screens see it: a snapshot of
/// its names taken when it was opened. Open it again after a sync or an
/// edit; that is one directory walk.
#[derive(uniffi::Object)]
pub struct PassStore {
    store: StoreDirs,
    tree: StoreTree,
}

#[uniffi::export]
impl PassStore {
    /// The store kept in `state_dir`, checked out in its `repo/`.
    #[uniffi::constructor(name = "new")]
    pub fn open(state_dir: String) -> Result<Arc<Self>, PawError> {
        let store = StoreDirs::new(Path::new(&state_dir));
        // With the store lock shared: a checkout is never seen half done.
        let tree = {
            let _reading = store.shared()?;
            StoreTree::scan(store.repo_dir())?
        };
        Ok(Arc::new(Self { store, tree }))
    }

    pub fn home(&self) -> FolderListing {
        self.tree.folder("").cloned().unwrap_or(FolderListing {
            path: String::new(),
            folders: Vec::new(),
            entries: Vec::new(),
        })
    }

    pub fn folder(&self, path: String) -> Result<FolderListing, PawError> {
        self.tree
            .folder(&path)
            .cloned()
            .ok_or_else(|| PawError::Store(format!("there is no folder called {path}")))
    }

    /// Every entry in the store, sorted by path.
    pub fn all_entries(&self) -> Vec<EntryRef> {
        self.tree.entries().to_vec()
    }

    /// Every folder that holds entries, sorted by path; not the top.
    pub fn all_folders(&self) -> Vec<String> {
        self.tree.folders()
    }

    pub fn entry(&self, path: String) -> Option<EntryRef> {
        let (folder, _) = crate::path::split(&path);
        self.tree.folder(folder)?.entries.iter().find(|entry| entry.path == path).cloned()
    }

    pub fn entry_count(&self) -> u32 {
        self.tree.entries().len() as u32
    }

    pub fn search(&self, query: String, limit: u32) -> Vec<SearchHit> {
        search::search(self.tree.entries(), &query, limit)
    }

    /// The entries AutoFill offers for the services iOS names; see
    /// [`service::match_services`].
    pub fn match_services(&self, services: Vec<String>, pairings: Option<Arc<SitePairings>>) -> Vec<MatchedEntry> {
        let known = pairings.map(|p| p.pairings()).unwrap_or_default();
        service::match_services(self.tree.entries(), &services, &known)
    }

    /// The keys `.gpg-id` names for entries in `folder`, as written there.
    pub fn recipients(&self, folder: String) -> Result<Vec<String>, PawError> {
        let _reading = self.store.shared()?;
        gpg_id::recipients_for(self.store.repo_dir(), &folder)
    }

    /// Whether the key whose decryption fingerprint is `card_fingerprint`
    /// (`CardInfo.decryption_fingerprint`) can open `entry`. Ask before
    /// asking for a PIN or a touch.
    pub fn access(&self, entry: String, card_fingerprint: String) -> Access {
        match self.read_entry(&entry) {
            Ok(bytes) => crate::recipients::access(&bytes, &card_fingerprint),
            Err(e) => Access::Unreadable { reason: e.to_string() },
        }
    }
}

impl PassStore {
    /// The encrypted bytes of `entry` ([`read::read_entry`]), read with the
    /// store lock shared (so never a file a checkout is halfway through
    /// writing) and released before anything slow, such as asking the key
    /// to decrypt.
    pub fn read_entry(&self, entry: &str) -> Result<Vec<u8>, PawError> {
        let _reading = self.store.shared()?;
        read::read_entry(self.store.repo_dir(), entry)
    }

    /// [`Self::access`] for a fingerprint already read.
    pub(crate) fn key_access(&self, entry: &str, card: &Fingerprint) -> Access {
        match self.read_entry(entry) {
            Ok(bytes) => crate::recipients::key_access(&bytes, Some(card)),
            Err(e) => Access::Unreadable { reason: e.to_string() },
        }
    }

    pub fn entries(&self) -> &[EntryRef] {
        self.tree.entries()
    }

    pub(crate) fn tree(&self) -> &StoreTree {
        &self.tree
    }
}
