//! Face ID copies: what is kept, for how long, and what ends them. Runs on
//! the simulated keystore and clock; the keystore really encrypts, so what
//! lands on disk is what a device would write.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use paw_core::PawError;
use paw_core::copies::format::{CopyFile, IndexKeys};
use paw_core::copies::{CopyCache, CopyStatus, DEFAULT_TTL, LiveCopy, MAX_TTL};
use paw_core::sim::copies::{SimClock, SimFaceId, SimKeystore};
use proptest::prelude::*;

const HOUR: u64 = 60 * 60 * 1000;
const START: i64 = 1_790_000_000_000;
const GITHUB: &str = "web/github.com";
const GITHUB_TEXT: &[u8] = b"tidal-Quartz-88-ferns\nlogin: you@example.com\n";
const BANK: &str = "bank/first-example-bank";
const BANK_TEXT: &[u8] = b"Harbor_73_willow\nlogin: 00417788\n";
const REASON: &str = "Open web/github.com";

struct World {
    dir: tempfile::TempDir,
    keystore: Arc<SimKeystore>,
    clock: Arc<SimClock>,
    cache: Arc<CopyCache>,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let keystore = SimKeystore::new();
        let clock = SimClock::new(START);
        let cache = CopyCache::new(window(&dir), keystore.clone(), clock.clone());
        Self { dir, keystore, clock, cache }
    }

    /// Another process (the AutoFill extension) on the same directory.
    fn other_process(&self) -> Arc<CopyCache> {
        CopyCache::new(window(&self.dir), self.keystore.clone(), self.clock.clone())
    }

    fn put(&self, entry: &str, text: &[u8], ttl: Duration) -> Option<i64> {
        self.cache.put(entry, text, b"v1", ttl).unwrap()
    }

    fn open(&self, entry: &str) -> Result<Vec<u8>, PawError> {
        self.cache.open(entry, b"v1", REASON).map(|text| text.to_vec())
    }

    fn status(&self, entry: &str) -> CopyStatus {
        self.cache.status(entry, b"v1").unwrap()
    }

    fn files(&self) -> Vec<(String, Vec<u8>)> {
        let window = Path::new(&window(&self.dir)).to_owned();
        let Ok(listing) = std::fs::read_dir(&window) else { return Vec::new() };
        let mut out: Vec<_> = listing
            .map(|child| {
                let child = child.unwrap();
                (child.file_name().into_string().unwrap(), std::fs::read(child.path()).unwrap())
            })
            .collect();
        out.sort();
        out
    }

    fn only_file(&self) -> (std::path::PathBuf, Vec<u8>) {
        let files = self.files();
        assert_eq!(files.len(), 1, "expected exactly one copy file");
        let (name, bytes) = files.into_iter().next().unwrap();
        (Path::new(&window(&self.dir)).join(name), bytes)
    }

    fn keys(&self) -> IndexKeys {
        IndexKeys::derive(&self.keystore.index_key().unwrap()).unwrap()
    }
}

fn window(dir: &tempfile::TempDir) -> String {
    dir.path().join("window").to_str().unwrap().to_owned()
}

fn hours(n: u64) -> Duration {
    Duration::from_millis(n * HOUR)
}

#[test]
fn copy_opens_until_its_deadline_and_not_after() {
    let w = World::new();
    let until = w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL).unwrap();
    assert_eq!(until, START + 8 * HOUR as i64);
    assert_eq!(w.status(GITHUB), CopyStatus::Available { until_ms: until });

    w.clock.advance_ms(8 * HOUR - 1);
    assert_eq!(w.open(GITHUB).unwrap(), GITHUB_TEXT);

    w.clock.advance_ms(1);
    assert!(matches!(w.open(GITHUB), Err(PawError::CopyUnavailable)));
    assert_eq!(w.status(GITHUB), CopyStatus::NeedsKey);
    assert_eq!(w.keystore.prompt_count(), 1);
}

#[test]
fn each_entry_keeps_its_own_deadline() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    w.clock.advance_ms(5 * HOUR);
    let bank_until = w.put(BANK, BANK_TEXT, DEFAULT_TTL).unwrap();
    assert_eq!(
        w.cache.list().unwrap(),
        vec![
            LiveCopy { entry: GITHUB.into(), until_ms: START + 8 * HOUR as i64 },
            LiveCopy { entry: BANK.into(), until_ms: bank_until },
        ],
        "soonest to end first"
    );

    w.clock.advance_ms(4 * HOUR);
    assert_eq!(w.status(GITHUB), CopyStatus::NeedsKey);
    assert_eq!(w.open(BANK).unwrap(), BANK_TEXT);
    assert_eq!(w.cache.list().unwrap().len(), 1);
}

#[test]
fn opening_by_face_id_never_extends_a_copy() {
    let w = World::new();
    let until = w.put(GITHUB, GITHUB_TEXT, hours(1)).unwrap();
    w.clock.advance_ms(HOUR / 2);
    w.open(GITHUB).unwrap();
    assert_eq!(w.status(GITHUB), CopyStatus::Available { until_ms: until });
}

#[test]
fn expired_copy_is_wiped_without_a_prompt() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, hours(1));
    w.clock.advance_ms(HOUR);
    assert!(matches!(w.open(GITHUB), Err(PawError::CopyUnavailable)));
    assert_eq!(w.keystore.prompt_count(), 0);
    assert!(w.files().is_empty());
}

#[test]
fn sweep_wipes_expired_copies_and_keeps_live_ones() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, hours(1));
    w.put(BANK, BANK_TEXT, hours(2));
    w.clock.advance_ms(HOUR);
    w.cache.sweep().unwrap();
    assert_eq!(w.files().len(), 1);
    assert_eq!(w.cache.list().unwrap()[0].entry, BANK);
}

#[test]
fn stale_copy_is_wiped_without_a_prompt() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    // A pull changed the entry's file.
    assert!(matches!(w.cache.open(GITHUB, b"v2", REASON), Err(PawError::CopyUnavailable)));
    assert_eq!(w.keystore.prompt_count(), 0);
    assert!(w.files().is_empty());

    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    assert_eq!(w.cache.status(GITHUB, b"v2").unwrap(), CopyStatus::Stale);
    assert_eq!(w.status(GITHUB), CopyStatus::NeedsKey);
}

#[test]
fn clock_moved_back_ends_every_copy() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    w.clock.advance_ms(HOUR);
    w.put(BANK, BANK_TEXT, DEFAULT_TTL);
    // Back to before the bank copy was made, though after GitHub's.
    w.clock.set_wall_ms(START + 1000);
    assert_eq!(w.status(GITHUB), CopyStatus::NeedsKey);
    assert!(w.files().is_empty());
    assert_eq!(w.keystore.reset_count(), 1);
}

#[test]
fn wall_clock_moved_back_a_little_still_cannot_lengthen_a_copy() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, hours(2));
    w.clock.advance_ms(HOUR);
    // Set back by less than an hour: still after the copy was made.
    w.clock.set_wall_ms(START + 1);
    w.clock.advance_ms(HOUR);
    // Two hours have really passed, though the wall clock says one.
    assert_eq!(w.status(GITHUB), CopyStatus::NeedsKey);
}

#[test]
fn reboot_ends_every_copy() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    w.put(BANK, BANK_TEXT, DEFAULT_TTL);
    w.clock.reboot();
    assert!(matches!(w.open(BANK), Err(PawError::CopyUnavailable)));
    assert_eq!(w.keystore.prompt_count(), 0);
    assert!(w.files().is_empty());
    assert!(w.cache.list().unwrap().is_empty());
}

#[test]
fn lock_now_ends_every_copy_and_resets_the_keystore() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    w.put(BANK, BANK_TEXT, DEFAULT_TTL);
    let (path, before) = w.only_file_named(GITHUB);
    w.cache.lock_now().unwrap();
    assert!(w.files().is_empty());
    assert_eq!(w.keystore.reset_count(), 1);

    // A copy that survived the delete (written by the other process at
    // that moment) can never be opened: its keys are gone.
    std::fs::write(&path, before).unwrap();
    assert!(matches!(w.open(GITHUB), Err(PawError::CopyUnavailable)));
    assert!(w.files().is_empty());
}

impl World {
    fn only_file_named(&self, entry: &str) -> (std::path::PathBuf, Vec<u8>) {
        let name = format!("{}.copy", self.keys().file_stem(entry));
        let path = Path::new(&window(&self.dir)).join(name);
        let bytes = std::fs::read(&path).unwrap();
        (path, bytes)
    }
}

#[test]
fn enrolment_change_clears_every_copy_with_its_own_error() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    w.put(BANK, BANK_TEXT, DEFAULT_TTL);
    w.keystore.change_enrolment();
    let err = w.open(GITHUB).unwrap_err();
    assert!(matches!(err, PawError::FaceIdChanged));
    assert_eq!(
        err.to_string(),
        "Face ID changed on this iPhone, so opened passwords were cleared"
    );
    assert!(w.files().is_empty());
    assert_eq!(w.keystore.reset_count(), 1);
    // The new key works.
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    assert_eq!(w.open(GITHUB).unwrap(), GITHUB_TEXT);
}

#[test]
fn face_id_cancel_leaves_the_copy_in_place() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    for (answer, expected) in [
        (SimFaceId::Cancel, "FaceIdCancelled"),
        (SimFaceId::NoMatch, "FaceIdNotRecognised"),
        (SimFaceId::LockedOut, "FaceIdLockedOut"),
        (SimFaceId::NotEnrolled, "FaceIdNotSetUp"),
    ] {
        w.keystore.set_face_id(answer);
        let err = w.open(GITHUB).unwrap_err();
        assert_eq!(format!("{err:?}"), expected);
        assert_eq!(w.files().len(), 1);
    }
    w.keystore.set_face_id(SimFaceId::Match);
    assert_eq!(w.open(GITHUB).unwrap(), GITHUB_TEXT);
}

#[test]
fn blob_moved_to_another_entrys_name_is_rejected() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    w.put(BANK, BANK_TEXT, DEFAULT_TTL);
    let (bank_path, _) = w.only_file_named(BANK);
    let (github_path, github_bytes) = w.only_file_named(GITHUB);
    std::fs::write(&bank_path, &github_bytes).unwrap();

    assert_eq!(w.status(BANK), CopyStatus::NeedsKey);
    assert!(matches!(w.open(BANK), Err(PawError::CopyUnavailable)));
    assert_eq!(w.keystore.prompt_count(), 0);
    assert!(!bank_path.exists());
    assert!(github_path.exists(), "the genuine copy is untouched");
}

#[test]
fn copy_spliced_from_another_entry_is_rejected_before_a_prompt() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    w.put(BANK, BANK_TEXT, DEFAULT_TTL);
    let (bank_path, bank_bytes) = w.only_file_named(BANK);
    let (_, github_bytes) = w.only_file_named(GITHUB);
    let keys = w.keys();
    // Keep the bank's sealed metadata, take GitHub's wrapped key and text.
    let mut spliced = CopyFile::parse(&keys, &github_bytes).unwrap();
    spliced.meta = CopyFile::parse(&keys, &bank_bytes).unwrap().meta;
    let bytes = spliced.to_bytes(&keys).unwrap();
    // Re-sealing with the index key makes the metadata itself valid, so the
    // text's own binding has to catch it, after Face ID.
    std::fs::write(&bank_path, bytes).unwrap();
    assert!(matches!(w.open(BANK), Err(PawError::CopyUnavailable)));
    assert!(!bank_path.exists());
}

#[test]
fn deadline_edited_on_disk_is_rejected() {
    // With only the files: any changed byte fails the metadata, before a
    // prompt.
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, hours(1));
    let (path, bytes) = w.only_file();
    for at in [8, 40, 60, bytes.len() - 1] {
        let mut edited = bytes.clone();
        edited[at] ^= 1;
        std::fs::write(&path, &edited).unwrap();
        assert_eq!(w.status(GITHUB), CopyStatus::NeedsKey);
        assert!(!path.exists());
    }
    assert_eq!(w.keystore.prompt_count(), 0);

    // Even with the index key (keychain access without Face ID), a new
    // deadline does not match what the text was sealed with.
    w.put(GITHUB, GITHUB_TEXT, hours(1));
    let (path, bytes) = w.only_file();
    let keys = w.keys();
    let mut file = CopyFile::parse(&keys, &bytes).unwrap();
    file.meta.deadline_wall_ms += 10 * HOUR as i64;
    std::fs::write(&path, file.to_bytes(&keys).unwrap()).unwrap();
    w.clock.advance_ms(2 * HOUR);
    // The monotonic clock still bounds it at the edited length...
    assert!(matches!(w.status(GITHUB), CopyStatus::Available { .. }));
    // ...but the text refuses to open, and the copy goes.
    assert!(matches!(w.open(GITHUB), Err(PawError::CopyUnavailable)));
    assert!(!path.exists());
}

#[test]
fn nothing_on_disk_contains_the_plaintext_or_the_entry_name() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    w.put(BANK, BANK_TEXT, DEFAULT_TTL);
    let needles: Vec<&[u8]> = vec![
        GITHUB.as_bytes(),
        BANK.as_bytes(),
        b"github",
        b"bank",
        b"tidal-Quartz",
        b"Harbor_73",
        b"login",
        b"sim-boot",
    ];
    let files = w.files();
    assert_eq!(files.len(), 2);
    for (name, bytes) in &files {
        for needle in &needles {
            assert!(!contains(name.as_bytes(), needle), "{name} names the entry");
            assert!(!contains(bytes, needle), "{name} contains {:?}", String::from_utf8_lossy(needle));
        }
    }
    // Same-size buckets: the two files are the same length.
    assert_eq!(files[0].1.len(), files[1].1.len());
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn copy_files_are_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    let (path, _) = w.only_file();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn ttl_zero_stores_nothing() {
    let w = World::new();
    assert_eq!(w.put(GITHUB, GITHUB_TEXT, Duration::ZERO), None);
    assert!(w.files().is_empty());

    // And opening with the key under "always ask" forgets an older copy.
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    w.put(GITHUB, GITHUB_TEXT, Duration::ZERO);
    assert!(w.files().is_empty());
}

#[test]
fn ttl_above_the_maximum_is_clamped() {
    let w = World::new();
    let until = w.put(GITHUB, GITHUB_TEXT, hours(48)).unwrap();
    assert_eq!(until, START + MAX_TTL.as_millis() as i64);
    w.clock.advance_ms(12 * HOUR);
    assert_eq!(w.status(GITHUB), CopyStatus::NeedsKey);
}

#[test]
fn a_key_open_renews_the_copy() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, hours(1));
    w.clock.advance_ms(HOUR / 2);
    let until = w.put(GITHUB, GITHUB_TEXT, hours(1)).unwrap();
    assert_eq!(until, START + (HOUR + HOUR / 2) as i64);
    assert_eq!(w.files().len(), 1);
}

#[test]
fn a_copy_made_by_one_process_opens_in_the_other() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    let extension = w.other_process();
    assert_eq!(extension.open(GITHUB, b"v1", REASON).unwrap().to_vec(), GITHUB_TEXT);
}

#[test]
fn files_that_are_not_copies_are_left_alone() {
    let w = World::new();
    w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
    let window = Path::new(&window(&w.dir)).to_owned();
    std::fs::write(window.join("key.json"), b"{}").unwrap();
    std::fs::write(window.join("junk.copy"), b"not a copy").unwrap();
    w.cache.sweep().unwrap();
    let names: Vec<String> = w.files().into_iter().map(|(name, _)| name).collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"key.json".to_owned()));
    w.cache.lock_now().unwrap();
    assert_eq!(w.files().len(), 1, "lock now leaves other files");
}

/// The app and the extension, hammering one directory at once. No call may
/// fail other than "needs the key", and no open may ever return another
/// entry's text.
#[test]
fn two_caches_on_one_directory_stay_consistent() {
    let w = World::new();
    let entries: Vec<(String, Vec<u8>)> =
        (0..6).map(|i| (format!("site-{i}"), format!("secret number {i}").into_bytes())).collect();
    let entries = Arc::new(entries);

    let run = |cache: Arc<CopyCache>, clock: Arc<SimClock>, seed: usize| {
        let entries = entries.clone();
        std::thread::spawn(move || {
            for round in 0..300 {
                let (entry, text) = &entries[(round * 7 + seed) % entries.len()];
                match round % 5 {
                    0 | 1 => {
                        cache.put(entry, text, b"v1", Duration::from_secs(60)).unwrap();
                    }
                    2 | 3 => match cache.open(entry, b"v1", "test") {
                        Ok(opened) => assert_eq!(&*opened, text, "an entry opened as another"),
                        Err(PawError::CopyUnavailable) => {}
                        Err(e) => panic!("open failed: {e:?}"),
                    },
                    _ => {
                        cache.sweep().unwrap();
                        clock.advance_ms(7_000);
                    }
                }
            }
        })
    };
    let app = run(w.cache.clone(), w.clock.clone(), 0);
    let extension = run(w.other_process(), w.clock.clone(), 3);
    app.join().unwrap();
    extension.join().unwrap();

    w.cache.sweep().unwrap();
    for (name, _) in w.files() {
        assert!(name.ends_with(".copy"), "left behind: {name}");
    }
    for copy in w.cache.list().unwrap() {
        let (_, text) = entries.iter().find(|(entry, _)| *entry == copy.entry).unwrap();
        assert_eq!(&*w.cache.open(&copy.entry, b"v1", "test").unwrap(), text);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Whatever is in a copy file, reading it never panics, and it is
    /// wiped rather than opened.
    #[test]
    fn arbitrary_bytes_in_a_copy_file_never_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..2048),
        keep_magic in any::<bool>(),
    ) {
        let w = World::new();
        w.put(GITHUB, GITHUB_TEXT, DEFAULT_TTL);
        let (path, _) = w.only_file();
        let mut bytes = bytes;
        if keep_magic {
            let mut with_magic = b"PAWCOPY1".to_vec();
            with_magic.extend(bytes);
            bytes = with_magic;
        }
        std::fs::write(&path, &bytes).unwrap();
        prop_assert_eq!(w.status(GITHUB), CopyStatus::NeedsKey);
        prop_assert!(matches!(w.open(GITHUB), Err(PawError::CopyUnavailable)));
        prop_assert!(w.cache.list().unwrap().is_empty());
        prop_assert_eq!(w.keystore.prompt_count(), 0);
    }

    /// The parser alone, on bytes that start like a copy.
    #[test]
    fn parsing_arbitrary_bytes_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let keys = IndexKeys::derive(&[9u8; 32]).unwrap();
        let mut file = b"PAWCOPY1".to_vec();
        file.extend(bytes);
        prop_assert!(CopyFile::parse(&keys, &file).is_err());
    }
}
