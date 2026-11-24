//! The key log: what it records, what it never records, its files and how
//! they merge.

#![cfg(feature = "sim")]

mod key_common;

use std::sync::{Arc, Mutex};

use key_common::*;
use paw_core::ApduTransport;
use paw_core::key::keylog::KEEP_LINES;
use paw_core::key::{clear_key_log, key_log, key_log_text};
use paw_core::sim::{SimTouch, SimulatedKey};

/// The simulated key behind a transport that keeps a copy of every data
/// byte that went either way, to look for in the log.
struct Recording {
    sim: Arc<SimulatedKey>,
    data: Mutex<Vec<Vec<u8>>>,
}

impl ApduTransport for Recording {
    fn transmit(&self, apdu: &[u8]) -> Result<Vec<u8>, String> {
        let response = self.sim.transmit(apdu)?;
        let mut data = self.data.lock().unwrap();
        if apdu.len() > 5 {
            data.push(apdu[5..].to_vec());
        }
        if response.len() > 2 {
            data.push(response[..response.len() - 2].to_vec());
        }
        Ok(response)
    }
}

fn hex(bytes: &[u8]) -> (String, String) {
    let upper: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    (upper.clone(), upper.to_lowercase())
}

/// A rig whose port logs to `dir` as `process`, plugged in.
fn logged_rig(dir: &std::path::Path, process: &str, decrypt: SimTouch) -> Rig {
    let rig = rig_unplugged(decrypt, SimTouch::Off);
    rig.port.set_key_log(dir.to_string_lossy().into_owned(), process.into());
    rig.port.attach(rig.sim.clone()).unwrap();
    rig
}

fn open_and_finish(rig: &Rig, entry: &str) {
    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(rig, entry, &driver);
    assert!(matches!(finish(&task), paw_core::key::KeyTaskState::Done { .. }));
}

fn dir_string(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().into_owned()
}

#[test]
fn a_pin_and_decipher_run_is_logged_with_no_pin_no_data_and_no_names() {
    let state = tempfile::tempdir().unwrap();
    let rig = rig_unplugged(SimTouch::EachUse, SimTouch::Off);
    rig.port.set_key_log(dir_string(&state), "app".into());
    let recording = Arc::new(Recording { sim: rig.sim.clone(), data: Mutex::new(Vec::new()) });
    rig.port.attach(recording.clone()).unwrap();

    open_and_finish(&rig, ENTRY);

    let session = rig.port.session().unwrap();
    let merged = key_log_text(dir_string(&state), 0);
    let ring = session.diagnostics(0);
    println!("{merged}");
    for wanted in [
        "session start",
        "task open: PIN held no, touch decryption each use, authentication off, starts at Pin",
        "→ VERIFY 00 82 (with PIN): 9000 ok",
        "touch watch at once (decryption slot: each use)",
        "→ PSO:DECIPHER: 9000 ok",
        "stage Touch",
        "stage Done",
        "PIN held",
    ] {
        assert!(merged.contains(wanted), "the log should have {wanted:?}:\n{merged}");
        assert!(ring.contains(wanted), "the session's ring should have {wanted:?}:\n{ring}");
    }

    let ciphertext = std::fs::read(rig.store_dir().join(format!("{ENTRY}.gpg"))).unwrap();
    let mut secrets: Vec<Vec<u8>> = recording.data.lock().unwrap().clone();
    secrets.push(ciphertext);
    secrets.push(SECRET.as_bytes().to_vec());
    for text in [&merged, &ring] {
        for words in [PIN, SECRET, ENTRY, "github", "phone-pin", &hex(PIN.as_bytes()).0] {
            assert!(!text.contains(words), "the log shows {words:?}");
        }
        for data in &secrets {
            for window in data.windows(4) {
                let (upper, lower) = hex(window);
                assert!(!text.contains(&upper) && !text.contains(&lower), "the log shows data {upper}");
            }
            for window in data.windows(6) {
                assert!(!text.as_bytes().windows(6).any(|w| w == window), "the log shows raw data");
            }
        }
    }
}

#[test]
fn the_session_ring_is_bounded() {
    let state = tempfile::tempdir().unwrap();
    let rig = logged_rig(state.path(), "app", SimTouch::Off);
    for _ in 0..30 {
        open_and_finish(&rig, OTHER_ENTRY);
    }
    let ring = rig.port.session().unwrap().diagnostics(0);
    assert_eq!(ring.lines().count(), paw_core::key::keylog::RING);
}

#[test]
fn a_process_file_is_cut_back_to_its_last_lines() {
    let state = tempfile::tempdir().unwrap();
    let rig = logged_rig(state.path(), "app", SimTouch::Off);
    // About 15 lines an open: well past the cap.
    for _ in 0..60 {
        open_and_finish(&rig, OTHER_ENTRY);
    }
    let file = state.path().join("key-log.app");
    let size = std::fs::metadata(&file).unwrap().len();
    assert!(size <= 41 * 1024, "{size} bytes");
    let lines = key_log(dir_string(&state), 0);
    assert!(lines.len() >= KEEP_LINES && lines.len() < 2 * KEEP_LINES + 100, "{} lines", lines.len());
    assert!(lines.last().unwrap().text.contains("stage Done"));
    assert!(!state.path().join("key-log.app.tmp").exists());
}

#[test]
fn two_processes_write_their_own_files_and_merge_by_time() {
    let state = tempfile::tempdir().unwrap();
    let app = logged_rig(state.path(), "app", SimTouch::Off);
    let fill = logged_rig(state.path(), "fill", SimTouch::Off);
    for _ in 0..3 {
        open_and_finish(&app, ENTRY);
        std::thread::sleep(std::time::Duration::from_millis(5));
        open_and_finish(&fill, OTHER_ENTRY);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(state.path().join("key-log.app").exists());
    assert!(state.path().join("key-log.fill").exists());

    let lines = key_log(dir_string(&state), 0);
    assert!(lines.windows(2).all(|pair| pair[0].at_ms <= pair[1].at_ms), "merged by time");
    // The processes take turns: app's first task, then fill's, then app's.
    let starts: Vec<&str> =
        lines.iter().filter(|l| l.text.contains(" task open:")).map(|l| l.process.as_str()).collect();
    assert_eq!(starts, ["app", "fill", "app", "fill", "app", "fill"]);
    // Within a process, the file's order is kept.
    let app_lines: Vec<String> =
        lines.iter().filter(|l| l.process == "app").map(|l| l.text.clone()).collect();
    let app_file = std::fs::read_to_string(state.path().join("key-log.app")).unwrap();
    let in_file: Vec<String> =
        app_file.lines().map(|l| l.split_once('\t').unwrap().1.to_string()).collect();
    assert_eq!(app_lines, in_file);

    let text = key_log_text(dir_string(&state), 0);
    assert!(text.lines().any(|l| l.contains(" fill ") && l.contains("→ SELECT")));
}

#[test]
fn clear_removes_every_process_file() {
    let state = tempfile::tempdir().unwrap();
    let app = logged_rig(state.path(), "app", SimTouch::Off);
    let fill = logged_rig(state.path(), "fill", SimTouch::Off);
    open_and_finish(&app, ENTRY);
    open_and_finish(&fill, ENTRY);
    std::fs::write(state.path().join("enrolled-key"), "other state").unwrap();

    clear_key_log(dir_string(&state)).unwrap();

    assert!(key_log(dir_string(&state), 0).is_empty());
    assert!(state.path().join("enrolled-key").exists(), "only the log is cleared");
    // It goes on from there.
    open_and_finish(&app, ENTRY);
    assert!(!key_log(dir_string(&state), 0).is_empty());
}

#[test]
fn removing_the_store_removes_the_log_and_the_log_never_makes_the_folder_again() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("store");
    std::fs::create_dir(&state).unwrap();
    let rig = logged_rig(&state, "app", SimTouch::Off);
    open_and_finish(&rig, ENTRY);
    assert!(state.join("key-log.app").exists());

    // What "Remove store from this iPhone" does: the state folder goes.
    std::fs::remove_dir_all(&state).unwrap();
    open_and_finish(&rig, ENTRY);

    assert!(!state.exists());
    assert!(key_log(state.to_string_lossy().into_owned(), 0).is_empty());
}

#[test]
fn loading_the_enrolled_key_starts_the_log_for_this_process() {
    let state = tempfile::tempdir().unwrap();
    let rig = rig_unplugged(SimTouch::Off, SimTouch::Off);
    assert_eq!(rig.port.key_log_dir(), None);
    rig.port.load_enrolled_key(dir_string(&state)).unwrap();
    assert_eq!(rig.port.key_log_dir(), Some(dir_string(&state)));
    rig.port.attach(rig.sim.clone()).unwrap();
    // Not inside an `.appex`: the app's file.
    assert!(state.path().join("key-log.app").exists());

    // A name set explicitly is kept.
    let other = tempfile::tempdir().unwrap();
    let rig = rig_unplugged(SimTouch::Off, SimTouch::Off);
    rig.port.set_key_log(dir_string(&other), "fill".into());
    rig.port.load_enrolled_key(dir_string(&state)).unwrap();
    assert_eq!(rig.port.key_log_dir(), Some(dir_string(&other)));
}

#[test]
fn a_silent_reverify_is_logged_with_the_card_check() {
    let state = tempfile::tempdir().unwrap();
    let rig = logged_rig(state.path(), "app", SimTouch::EachUse);
    open_and_finish(&rig, ENTRY);
    // The card forgets the PIN on the next SELECT, which every job sends.
    rig.sim.set_reselect_drops_pin(true);
    open_and_finish(&rig, OTHER_ENTRY);
    let text = key_log_text(dir_string(&state), 0);
    // A silent re-verify, not a refusal: the job checked first.
    assert!(text.contains("→ VERIFY 00 82 (check): 63C3 not verified, 3 tries left"), "{text}");
    assert!(!text.contains("refused"));
}
