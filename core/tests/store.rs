//! Reading the names in a store, and staying inside it. The store is a
//! directory that came out of a git clone, so the tests plant what a hostile
//! or careless repository could contain.

use std::os::unix::fs::symlink;
use std::path::Path;
use std::time::{Duration, Instant};

use paw_core::PawError;
use paw_core::store::PassStore;
use paw_core::store::gpg_id::{parse_gpg_id, recipients_for};
use paw_core::store::read::read_entry;
use paw_core::store::tree::StoreTree;

fn put(root: &Path, path: &str, contents: &str) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, contents).unwrap();
}

/// A store with the things a real one has around its entries.
fn store() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for entry in [
        "Home Wi-Fi",
        "phone-pin",
        "web/github.com",
        "web/Example.org",
        "web/apple.com",
        "email/fastmail",
        "Bank/first-example-bank",
        "servers/example.net/root",
        "servers/example.net/deploy",
        "servers/backup-box",
    ] {
        put(root, &format!("{entry}.gpg"), "ciphertext");
    }
    put(root, ".gpg-id", "0123456789ABCDEF0123456789ABCDEF01234567\n");
    put(root, ".gitattributes", "*.gpg diff=gpg\n");
    put(root, ".git/config", "[core]\n");
    put(root, ".git/objects/ab/cdef.gpg", "not an entry");
    put(root, ".extensions/tool.bash", "#!/bin/sh\n");
    put(root, ".hidden.gpg", "not an entry");
    put(root, ".trash/old.gpg", "not an entry");
    put(root, "README.md", "not an entry");
    put(root, "web/notes.txt", "not an entry");
    put(root, "web/.gpg", "no name");
    std::fs::create_dir_all(root.join("empty/nested")).unwrap();
    dir
}

fn names<T>(items: &[T], name: impl Fn(&T) -> &str) -> Vec<&str> {
    items.iter().map(name).collect()
}

#[test]
fn lists_folders_first_then_entries_ignoring_letter_case() {
    let dir = store();
    let tree = StoreTree::scan(dir.path()).unwrap();

    let home = tree.folder("").unwrap();
    assert_eq!(names(&home.folders, |f| &f.name), ["Bank", "email", "servers", "web"]);
    assert_eq!(names(&home.entries, |e| &e.name), ["Home Wi-Fi", "phone-pin"]);

    let web = tree.folder("web").unwrap();
    assert!(web.folders.is_empty());
    assert_eq!(names(&web.entries, |e| &e.name), ["apple.com", "Example.org", "github.com"]);
}

#[test]
fn an_entry_has_its_path_its_name_and_its_folder() {
    let dir = store();
    let tree = StoreTree::scan(dir.path()).unwrap();

    let top = &tree.folder("").unwrap().entries[0];
    assert_eq!((top.path.as_str(), top.name.as_str(), top.folder.as_str()), ("Home Wi-Fi", "Home Wi-Fi", ""));

    let nested = &tree.folder("servers/example.net").unwrap().entries[1];
    assert_eq!(
        (nested.path.as_str(), nested.name.as_str(), nested.folder.as_str()),
        ("servers/example.net/root", "root", "servers/example.net")
    );
}

#[test]
fn a_folder_counts_the_entries_below_it_too() {
    let dir = store();
    let tree = StoreTree::scan(dir.path()).unwrap();

    let counts: Vec<_> = tree
        .folder("")
        .unwrap()
        .folders
        .iter()
        .map(|f| (f.path.as_str(), f.entry_count))
        .collect();
    assert_eq!(counts, [("Bank", 1), ("email", 1), ("servers", 3), ("web", 3)]);
    assert_eq!(tree.folder("servers").unwrap().folders[0].entry_count, 2);
    assert_eq!(tree.entries().len(), 10);
}

#[test]
fn hidden_files_and_files_that_are_not_gpg_are_not_entries() {
    let dir = store();
    let tree = StoreTree::scan(dir.path()).unwrap();

    let all: Vec<&str> = tree.entries().iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        all,
        [
            "Bank/first-example-bank",
            "email/fastmail",
            "Home Wi-Fi",
            "phone-pin",
            "servers/backup-box",
            "servers/example.net/deploy",
            "servers/example.net/root",
            "web/apple.com",
            "web/Example.org",
            "web/github.com",
        ]
    );
    for hidden in [".git", ".git/objects", ".extensions", ".trash"] {
        assert!(tree.folder(hidden).is_none(), "{hidden}");
    }
    // Hidden things are not "skipped": they are simply not the store's.
    assert!(tree.skipped().is_empty());
}

#[test]
fn folders_without_entries_are_not_listed() {
    let dir = store();
    let tree = StoreTree::scan(dir.path()).unwrap();

    assert!(tree.folder("empty").is_none());
    assert!(tree.folder("empty/nested").is_none());
    assert!(tree.folder("no-such-folder").is_none());
}

#[test]
fn an_empty_store_is_an_empty_home() {
    let dir = tempfile::tempdir().unwrap();
    let store = PassStore::open(dir.path().to_str().unwrap().into()).unwrap();

    assert_eq!(store.entry_count(), 0);
    assert!(store.home().folders.is_empty() && store.home().entries.is_empty());
    assert!(matches!(store.folder("web".into()), Err(PawError::Store(_))));
}

#[test]
fn a_store_that_is_not_there_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nowhere");
    assert!(matches!(StoreTree::scan(&missing), Err(PawError::Store(_))));
}

#[test]
fn lists_five_thousand_entries_quickly() {
    let dir = tempfile::tempdir().unwrap();
    for folder in 0..50 {
        let path = dir.path().join(format!("group-{folder:02}/site"));
        std::fs::create_dir_all(&path).unwrap();
        for entry in 0..100 {
            std::fs::write(path.join(format!("entry-{entry:03}.example.com.gpg")), b"").unwrap();
        }
    }

    let started = Instant::now();
    let store = PassStore::open(dir.path().to_str().unwrap().into()).unwrap();
    let scanned = started.elapsed();
    assert_eq!(store.entry_count(), 5000);
    assert_eq!(store.home().folders.len(), 50);
    assert_eq!(store.home().folders[7].entry_count, 100);

    let started = Instant::now();
    let hits = store.search("entry 042 group".into(), 50);
    let offered = store.match_services(vec!["https://entry-042.example.com/login".into()], None);
    let searched = started.elapsed();
    assert_eq!(hits.len(), 50);
    assert_eq!(offered.len(), 5000);
    let exact = |m: &paw_core::store::service::MatchedEntry| m.kind == paw_core::store::service::MatchKind::ExactHost;
    assert!(offered[..50].iter().all(exact) && !exact(&offered[50]));

    // Generous: a debug build on a busy machine takes a small part of this.
    assert!(scanned < Duration::from_secs(2), "scan took {scanned:?}");
    assert!(searched < Duration::from_secs(2), "search and matching took {searched:?}");
}

/// A store with links that lead outside it, next to a secret they point at.
struct Planted {
    dir: tempfile::TempDir,
}

impl Planted {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        put(&outside, "secret.gpg", "OUTSIDE SECRET");
        put(&outside, "folder/inner.gpg", "OUTSIDE SECRET");
        put(&outside, ".gpg-id", "attacker@example.com\n");

        let store = dir.path().join("store");
        put(&store, "web/github.com.gpg", "inside");
        put(&store, ".gpg-id", "you@example.com\n");
        symlink(outside.join("secret.gpg"), store.join("linked-file.gpg")).unwrap();
        symlink(outside.join("secret.gpg"), store.join("web/linked-file.gpg")).unwrap();
        symlink(outside.join("folder"), store.join("linked-folder")).unwrap();
        symlink(&outside, store.join("web/linked-folder")).unwrap();
        // Links that stay inside the store are refused all the same.
        symlink(store.join("web"), store.join("alias")).unwrap();
        // A link that points nowhere.
        symlink(store.join("gone.gpg"), store.join("dangling.gpg")).unwrap();
        Self { dir }
    }

    fn store(&self) -> std::path::PathBuf {
        self.dir.path().join("store")
    }
}

#[test]
fn links_are_never_listed_or_followed() {
    let planted = Planted::new();
    let tree = StoreTree::scan(&planted.store()).unwrap();

    let all: Vec<&str> = tree.entries().iter().map(|e| e.path.as_str()).collect();
    assert_eq!(all, ["web/github.com"]);
    assert_eq!(names(&tree.folder("").unwrap().folders, |f| &f.name), ["web"]);
    for linked in ["linked-folder", "web/linked-folder", "alias"] {
        assert!(tree.folder(linked).is_none(), "{linked}");
    }
    assert_eq!(
        tree.skipped(),
        [
            "alias",
            "dangling.gpg",
            "linked-file.gpg",
            "linked-folder",
            "web/linked-file.gpg",
            "web/linked-folder"
        ]
    );
}

#[test]
fn reading_an_entry_refuses_every_way_out_of_the_store() {
    let planted = Planted::new();
    let store = planted.store();
    let outside = planted.dir.path().join("outside");

    assert_eq!(read_entry(&store, "web/github.com").unwrap(), b"inside");

    let absolute = outside.join("secret").to_str().unwrap().to_owned();
    for path in [
        "linked-file",
        "web/linked-file",
        "linked-folder/inner",
        "web/linked-folder/secret",
        "web/linked-folder/folder/inner",
        "alias/github.com",
        "dangling",
        "../outside/secret",
        "web/../../outside/secret",
        "web/./github.com",
        "web//github.com",
        absolute.as_str(),
        "/etc/passwd",
        ".git/config",
        ".GIT/config",
        "web/.git/config",
        "git~1/config",
        ".hidden",
        "web\\github.com",
        "web/github.com\0",
        "",
    ] {
        match read_entry(&store, path) {
            Err(PawError::UnsafePath(_)) => {}
            other => panic!("{path:?} should be refused as unsafe, got {other:?}"),
        }
    }
}

#[test]
fn nothing_outside_the_store_is_read_through_the_store_object() {
    let planted = Planted::new();
    let store = PassStore::open(planted.store().to_str().unwrap().into()).unwrap();

    assert_eq!(store.entry_count(), 1);
    assert_eq!(StoreTree::scan(&planted.store()).unwrap().skipped().len(), 6);
    for path in ["linked-file", "linked-folder/inner", "web/linked-folder/secret", "../outside/secret"] {
        let refused = store.read_entry(path).unwrap_err();
        assert!(matches!(refused, PawError::UnsafePath(_)), "{path}: {refused:?}");
        // The reason names the path and never the linked file's contents.
        let paw_core::recipients::Access::Unreadable { reason } =
            store.access(path.into(), "0123456789ABCDEF0123456789ABCDEF01234567".into())
        else {
            panic!("{path} should be unreadable");
        };
        assert!(!reason.contains("OUTSIDE"), "{reason}");
    }
    // A `.gpg-id` reached through a linked folder is not consulted either.
    assert!(matches!(
        store.recipients("web/linked-folder".into()),
        Err(PawError::UnsafePath(_))
    ));
}

#[test]
fn a_missing_entry_is_not_found_rather_than_unsafe() {
    let dir = store();
    for path in ["web/nothing", "nowhere/at/all", "web/github.com/below-a-file", "web"] {
        match read_entry(dir.path(), path) {
            Err(PawError::Store(message)) => assert!(message.contains(path), "{message}"),
            other => panic!("{path:?}: {other:?}"),
        }
    }
}

#[test]
fn a_file_too_large_to_be_an_entry_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("huge.gpg"), vec![0u8; (1 << 20) + 1]).unwrap();
    std::fs::write(dir.path().join("large.gpg"), vec![0u8; 1 << 20]).unwrap();

    assert!(matches!(read_entry(dir.path(), "huge"), Err(PawError::Store(_))));
    assert_eq!(read_entry(dir.path(), "large").unwrap().len(), 1 << 20);
}

#[test]
fn gpg_id_lines_are_key_names_without_comments_or_blanks() {
    let text = "# the team\n\n0123456789ABCDEF0123456789ABCDEF01234567\n  you@example.com  \n\
                0xDEADBEEFDEADBEEF # the backup key\r\n\t\n#\nSecond Person <second@example.com>\n";
    assert_eq!(
        parse_gpg_id(text),
        [
            "0123456789ABCDEF0123456789ABCDEF01234567",
            "you@example.com",
            "0xDEADBEEFDEADBEEF",
            "Second Person <second@example.com>"
        ]
    );
    assert!(parse_gpg_id("").is_empty());
    assert_eq!(parse_gpg_id("no-newline-at-the-end"), ["no-newline-at-the-end"]);
}

#[test]
fn the_nearest_gpg_id_above_a_folder_wins() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    put(root, ".gpg-id", "top@example.com\n");
    put(root, "work/.gpg-id", "work@example.com\nboss@example.com\n");
    put(root, "work/team/deep/entry.gpg", "x");
    put(root, "work/private/.gpg-id", "me@example.com\n");
    put(root, "web/github.com.gpg", "x");

    let cases: [(&str, &[&str]); 8] = [
        ("", &["top@example.com"]),
        ("web", &["top@example.com"]),
        ("work", &["work@example.com", "boss@example.com"]),
        ("work/team", &["work@example.com", "boss@example.com"]),
        ("work/team/deep", &["work@example.com", "boss@example.com"]),
        ("work/private", &["me@example.com"]),
        // A folder that does not exist yet, as when adding an entry.
        ("work/new/folder", &["work@example.com", "boss@example.com"]),
        ("brand-new", &["top@example.com"]),
    ];
    for (folder, expected) in cases {
        assert_eq!(recipients_for(root, folder).unwrap(), expected, "{folder:?}");
    }
}

#[test]
fn a_store_without_gpg_id_says_so() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "web/github.com.gpg", "x");
    match recipients_for(dir.path(), "web") {
        Err(PawError::Store(message)) => assert!(message.contains(".gpg-id"), "{message}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn recipients_are_not_read_through_links_or_from_outside() {
    let planted = Planted::new();
    let store = planted.store();
    for folder in ["linked-folder", "web/linked-folder/folder", "alias", "..", "../outside", "/etc", ".git"] {
        match recipients_for(&store, folder) {
            Err(PawError::UnsafePath(_)) => {}
            other => panic!("{folder:?}: {other:?}"),
        }
    }

    // A `.gpg-id` that is itself a link is refused, not skipped in favour
    // of the one above it.
    put(&store, "work/entry.gpg", "x");
    symlink(planted.dir.path().join("outside/.gpg-id"), store.join("work/.gpg-id")).unwrap();
    assert!(matches!(recipients_for(&store, "work"), Err(PawError::UnsafePath(_))));
    assert_eq!(recipients_for(&store, "web").unwrap(), ["you@example.com"]);
}
