//! A git host for the sync tests: the simulated host of `paw_core::sim`
//! (an in-process SSH server with git's upload-pack and receive-pack written
//! on libgit2), plus a "desktop" that changes its repository with the real
//! `git` program. Nothing leaves the machine.

// Each test binary uses its own part of this.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use paw_core::sim::remote::{self, SimRemote, SimRemoteContents, SimRemoteOptions};
use paw_core::sync::Remote;

/// A phone's store in a temporary state directory, as setup lays it out:
/// the clone goes in its `repo/`.
pub struct Phone {
    _state: tempfile::TempDir,
    pub store: paw_core::files::layout::StoreDirs,
}

impl Phone {
    pub fn new() -> Self {
        let state = tempfile::tempdir().unwrap();
        let store = paw_core::files::layout::StoreDirs::new(state.path());
        Self { _state: state, store }
    }

    /// The working copy.
    pub fn path(&self) -> &Path {
        self.store.repo_dir()
    }

    pub fn state(&self) -> &Path {
        self.store.state_dir()
    }
}

/// Adds to the pinned recipients in `state_dir` the record that the
/// `.gpg-id` line `line` means the pinned key `fingerprint`, as setup
/// writes it after its test decrypt.
pub fn bind_recipient(state_dir: &Path, line: &str, fingerprint: &str) {
    use base64::Engine;
    let file = state_dir.join(paw_core::files::layout::RECIPIENTS);
    let mut text = std::fs::read_to_string(&file).unwrap();
    let line = base64::engine::general_purpose::STANDARD.encode(line.trim());
    text.push_str(&format!("bind {fingerprint} {line}\n"));
    std::fs::write(file, text).unwrap();
}

/// Runs `git` with the developer's own configuration out of the picture, so
/// the tests neither depend on it nor sign anything with a real key.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = isolated(Command::new("git"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git is installed");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn isolated(mut command: Command) -> Command {
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Desktop")
        .env("GIT_AUTHOR_EMAIL", "desktop@example.com")
        .env("GIT_COMMITTER_NAME", "Desktop")
        .env("GIT_COMMITTER_EMAIL", "desktop@example.com");
    command
}

/// A public key line for a host key the server does not hold.
pub fn some_other_host_key() -> String {
    remote::changed_host_key()
}

/// The public key line of the server's ed25519 host key.
pub fn ed25519_host_key() -> String {
    remote::ed25519_host_key()
}

/// The public key line of the ECDSA host key a server started with
/// [`GitServer::start_with_two_host_keys`] also holds.
pub fn ecdsa_host_key() -> String {
    remote::ecdsa_host_key()
}

/// The simulated host, plus what the tests do to it from "the desktop"
/// with the `git` program.
pub struct GitServer {
    pub sim: Arc<SimRemote>,
}

impl GitServer {
    /// Starts a server holding one bare repository with a single commit that
    /// adds `README`. Only the keys in `authorised` (public key lines) may
    /// log in.
    pub fn start(authorised: &[String]) -> Self {
        Self::start_with(authorised, readme(), false)
    }

    /// A server whose repository has no commits at all.
    pub fn start_empty(authorised: &[String]) -> Self {
        Self::start_with(authorised, SimRemoteContents::Empty, false)
    }

    /// Like [`Self::start`], with an ECDSA host key besides the ed25519 one.
    pub fn start_with_two_host_keys(authorised: &[String]) -> Self {
        Self::start_with(authorised, readme(), true)
    }

    /// A server holding the fixture store.
    pub fn start_with_fixture(authorised: &[String]) -> Self {
        Self::start_with(authorised, SimRemoteContents::FixtureStore, false)
    }

    fn start_with(authorised: &[String], contents: SimRemoteContents, ecdsa_host_key: bool) -> Self {
        let sim = SimRemote::start_with(SimRemoteOptions { authorised: authorised.to_vec(), contents, ecdsa_host_key })
            .unwrap();
        Self { sim }
    }

    pub fn remote(&self) -> Remote {
        self.sim.remote().unwrap()
    }

    pub fn bare(&self) -> PathBuf {
        self.sim.bare()
    }

    /// The commit `main` points at on the server.
    pub fn tip(&self) -> String {
        git(&self.bare(), &["rev-parse", "refs/heads/main"])
    }

    /// Adds a commit to the server's `main` the way another machine would:
    /// from a separate clone, over the file system.
    pub fn desktop_commit(&self, path: &str, contents: &str) -> String {
        self.desktop_change(path, |file| std::fs::write(file, contents).unwrap())
    }

    /// Commits a symbolic link named `path` that points at `target`, as a
    /// hostile or careless collaborator could.
    pub fn desktop_symlink(&self, path: &str, target: &Path) -> String {
        self.desktop_change(path, |link| std::os::unix::fs::symlink(target, link).unwrap())
    }

    fn desktop_change(&self, path: &str, create: impl FnOnce(&Path)) -> String {
        desktop_change_at(&self.bare(), path, create);
        self.tip()
    }

    /// A fresh clone of the server's repository on "the desktop", for tests
    /// that change it with `git` and push it themselves.
    pub fn desktop(&self) -> tempfile::TempDir {
        let work = tempfile::tempdir().unwrap();
        git(work.path(), &["clone", "--quiet", self.bare().to_str().unwrap(), "."]);
        work
    }

    /// Has the desktop push a commit writing `path` while the next push from
    /// the phone is already under way: after its connection is open, before
    /// the host lists its branches.
    pub fn move_before_next_push(&self, path: &str, contents: &str) {
        let (bare, path, contents) = (self.bare(), path.to_owned(), contents.to_owned());
        self.sim.before_next_push(move || {
            desktop_change_at(&bare, &path, |file| std::fs::write(file, contents).unwrap());
        });
    }

    pub fn logins(&self) -> u32 {
        self.sim.logins()
    }

    /// The git services each logged-in connection ran, in login order.
    pub fn services(&self) -> Vec<Vec<String>> {
        self.sim.services()
    }
}

fn readme() -> SimRemoteContents {
    SimRemoteContents::Files(vec![("README".into(), b"a password store\n".to_vec())])
}

fn desktop_change_at(bare: &Path, path: &str, create: impl FnOnce(&Path)) {
    let work = tempfile::tempdir().unwrap();
    git(work.path(), &["clone", "--quiet", bare.to_str().unwrap(), "."]);
    let file = work.path().join(path);
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder).unwrap();
    }
    create(&file);
    git(work.path(), &["add", path]);
    git(work.path(), &["commit", "--quiet", "-m", &format!("Add {path} from the desktop.")]);
    git(work.path(), &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
}

/// Moves `from` to `to` (full paths) as the edit form does: an edit of the
/// open entry whose name and folder change, saved. `open` is its text;
/// without it a placeholder stands in, for a move that keeps the file as it
/// is (the same recipients), and the helper fails if the move re-encrypted.
pub fn move_entry(
    store: &paw_core::write::WriteStore,
    from: &str,
    to: &str,
    open: Option<&paw_core::entry::EntryDoc>,
) -> Result<paw_core::write::Saved, paw_core::write::WriteError> {
    use paw_core::entry::EntryDoc;
    let placeholder = open.is_none();
    let doc = match open {
        Some(doc) => doc.duplicate(),
        None => EntryDoc::parse(b"placeholder\n".to_vec()).unwrap(),
    };
    let before = std::fs::read(store.dirs().repo_dir().join(format!("{from}.gpg"))).ok();
    let id = paw_core::files::content_id(before.as_deref().unwrap_or_default());
    let mut draft = paw_core::write::Draft::edit(from, doc, id);
    let (folder, name) = to.rsplit_once('/').unwrap_or(("", to));
    draft.set_folder(folder);
    draft.set_name(name);
    let saved = store.save(&draft)?;
    if placeholder {
        let after = std::fs::read(store.dirs().repo_dir().join(format!("{to}.gpg"))).ok();
        assert_eq!(before, after, "a move without the open entry re-encrypted a placeholder");
    }
    Ok(saved)
}
