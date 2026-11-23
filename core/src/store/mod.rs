//! Reading a password store that is checked out on this device.
//!
//! Everything here works from names and from the unencrypted start of each
//! file. Nothing in this module decrypts, and nothing needs the key or the
//! network. The directory is treated as hostile input throughout: see
//! [`read`] for how paw stays inside it.

pub mod gpg_id;
pub mod read;
pub mod search;
pub mod service;
pub mod tree;

use std::path::PathBuf;
use std::sync::Arc;

use crate::PawError;
use crate::recipients::{Access, EntryRecipients};
use search::SearchHit;
use service::ServiceMatch;
use tree::{EntryRef, FolderListing, StoreTree};

/// A password store on this device, as the screens see it: a snapshot of
/// its names taken when it was opened. Open it again after a sync or an
/// edit; that is one directory walk.
#[derive(uniffi::Object)]
pub struct PassStore {
    root: PathBuf,
    tree: StoreTree,
}

impl PassStore {
    /// The state directory of a store set up by paw: the folder holding
    /// this checkout, when the checkout is its `repo` folder (as
    /// `setup::repo_dir` makes it). `None` for a store opened anywhere else,
    /// such as the fixture store.
    pub(crate) fn state_dir(&self) -> Option<&std::path::Path> {
        let name = self.root.file_name()?;
        if name != std::ffi::OsStr::new("repo") {
            return None;
        }
        self.root.parent()
    }

    /// For the per-call paths that take a card and a PIN: refuses, before
    /// any VERIFY, a card other than the store's enrolled key.
    pub(crate) fn check_card(&self, card: &std::sync::Arc<dyn crate::ApduTransport>) -> Result<(), PawError> {
        match self.state_dir() {
            Some(state_dir) => crate::key::enrolment::check_card(state_dir, card).map_err(|r| r.into_paw_error()),
            None => Ok(()),
        }
    }
}

#[uniffi::export]
impl PassStore {
    /// Reads the names in the store checked out at `dir`.
    #[uniffi::constructor(name = "new")]
    pub fn open(dir: String) -> Result<Arc<Self>, PawError> {
        let root = PathBuf::from(dir);
        let tree = StoreTree::scan(&root)?;
        Ok(Arc::new(Self { root, tree }))
    }

    /// The top of the store, for Home.
    pub fn home(&self) -> FolderListing {
        self.tree.folder("").cloned().unwrap_or(FolderListing {
            path: String::new(),
            folders: Vec::new(),
            entries: Vec::new(),
        })
    }

    /// The contents of one folder, e.g. `web`.
    pub fn folder(&self, path: String) -> Result<FolderListing, PawError> {
        self.tree
            .folder(&path)
            .cloned()
            .ok_or_else(|| PawError::Store(format!("there is no folder called {path}")))
    }

    /// How many entries the store holds.
    pub fn entry_count(&self) -> u32 {
        self.tree.entries().len() as u32
    }

    /// Paths left out of the store because they are links or otherwise not
    /// safe to read. For diagnostics.
    pub fn skipped(&self) -> Vec<String> {
        self.tree.skipped().to_vec()
    }

    /// Searches entry and folder names; see [`search::search`].
    pub fn search(&self, query: String, limit: u32) -> Vec<SearchHit> {
        search::search(self.tree.entries(), &query, limit)
    }

    /// The entries to offer for a website or app; see
    /// [`service::match_service`].
    pub fn match_service(&self, service: String) -> Vec<ServiceMatch> {
        service::match_service(self.tree.entries(), &service)
    }

    /// The keys `.gpg-id` names for entries in `folder`, as written there.
    pub fn recipients(&self, folder: String) -> Result<Vec<String>, PawError> {
        gpg_id::recipients_for(&self.root, &folder)
    }

    /// The keys the entry file itself says it is encrypted for. Nothing is
    /// decrypted.
    pub fn entry_recipients(&self, entry: String) -> Result<EntryRecipients, PawError> {
        crate::recipients::entry_recipients(&self.read_entry(&entry)?)
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
    /// The encrypted bytes of `entry`; see [`read::read_entry`].
    pub fn read_entry(&self, entry: &str) -> Result<Vec<u8>, PawError> {
        read::read_entry(&self.root, entry)
    }

    /// Every entry, sorted by path.
    pub fn entries(&self) -> &[EntryRef] {
        self.tree.entries()
    }
}
