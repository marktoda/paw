//! The key log: what it records, what it never records, its files and how
//! they merge.

#![cfg(feature = "sim")]

mod key_common;
mod support;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use key_common::*;
use paw_core::key::CardConnection;
use paw_core::key::keylog::KEEP_LINES;
use paw_core::ffi::sync::Setup;
use paw_core::key::{KeyError, KeyPort, clear_key_log, key_log, key_log_text};
use paw_core::setup::SetupState;
use paw_core::sim::copies::SimClock;
use paw_core::task::{KeyOutcome, KeyTaskState};
use paw_core::sim::{SimOptions, SimTouch, SimulatedKey};

/// The simulated key behind a transport that keeps a copy of every data
/// byte that went either way, to look for in the log. `losses` times, it
/// makes the card forget the PIN just before a key operation.
struct Recording {
    sim: Arc<SimulatedKey>,
    data: Mutex<Vec<Vec<u8>>>,
    losses: AtomicU32,
}

impl Recording {
    fn new(sim: &Arc<SimulatedKey>) -> Arc<Self> {
        Arc::new(Self { sim: sim.clone(), data: Mutex::new(Vec::new()), losses: AtomicU32::new(0) })
    }
}

impl CardConnection for Recording {
    fn transmit(&self, apdu: Vec<u8>) -> Result<Vec<u8>, paw_core::key::ConnectionError> {
        let key_operation = matches!(apdu.as_slice(), [cla, 0x2A, 0x80, 0x86, ..] | [cla, 0x88, ..] if cla & 0x10 == 0);
        if key_operation && self.losses.load(Ordering::SeqCst) > 0 && self.losses.fetch_sub(1, Ordering::SeqCst) > 0 {
            self.sim.transmit(vec![0x00, 0x20, 0xFF, 0x82])?;
        }
        let response = self.sim.transmit(apdu.clone())?;
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
    rig.port.plugged_in(rig.sim.clone()).unwrap();
    rig
}

fn open_and_finish(rig: &Rig, entry: &str) {
    let driver = Driver::helpful(PIN, &rig.sim);
    let task = open(rig, entry, &driver);
    assert!(matches!(finish(&task), paw_core::task::KeyTaskState::Done { .. }));
}

fn dir_string(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().into_owned()
}

#[test]
fn a_pin_and_decipher_run_is_logged_with_no_pin_no_data_and_no_names() {
    let state = tempfile::tempdir().unwrap();
    let rig = rig_unplugged(SimTouch::EachUse, SimTouch::Off);
    rig.port.set_key_log(dir_string(&state), "app".into());
    let recording = Recording::new(&rig.sim);
    rig.port.plugged_in(recording.clone()).unwrap();

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
    assert_clean(&[&merged, &ring], &recording, &[ciphertext, SECRET.as_bytes().to_vec()], &[]);
}

#[test]
fn a_sign_in_a_setup_check_and_a_touch_check_are_logged_with_no_pin_no_data_and_no_names() {
    let state = tempfile::tempdir().unwrap();
    let sim = key_common::sim(SimTouch::EachUse, SimTouch::EachUse, WAIT);
    let port = KeyPort::new(SimClock::new(1_790_000_000_000));
    port.set_key_log(dir_string(&state), "app".into());
    let recording = Recording::new(&sim);
    port.plugged_in(recording.clone()).unwrap();
    let server = support::GitServer::start_with_fixture(&[sim.ssh_public_key()]);
    let dir = tempfile::tempdir().unwrap();
    let setup = Setup::open(dir_string(&dir)).unwrap();
    setup.read_key_from_session(port.session().unwrap()).unwrap();
    setup.set_address(server.sim.address()).unwrap();
    let SetupState::ConfirmHost { fingerprint, .. } = setup.check_host().unwrap() else { panic!("expected ConfirmHost") };
    setup.confirm_host(fingerprint).unwrap();

    let driver = Driver::helpful(PIN, &sim);
    let task = setup.clone().download_with_key(port.clone(), driver.clone());
    driver.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Done { outcome: KeyOutcome::SignedIn }));
    let task = setup.clone().test_decrypt_with_key(port.clone(), driver.clone());
    driver.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Done { outcome: KeyOutcome::KeyChecked }));
    let task = port.clone().check_touch_timeout(driver.clone());
    driver.drive(&task);
    assert!(matches!(finish(&task), KeyTaskState::Done { outcome: KeyOutcome::SignedIn }));

    let merged = key_log_text(dir_string(&state), 0);
    let ring = port.session().unwrap().diagnostics(0);
    for wanted in ["task sign in to download:", "task setup check:", "task touch timeout check:", "→ INTERNAL AUTHENTICATE: 9000 ok", "→ PSO:DECIPHER: 9000 ok"] {
        assert!(merged.contains(wanted), "the log should have {wanted:?}:\n{merged}");
    }
    assert_clean(&[&merged, &ring], &recording, &[SECRET.as_bytes().to_vec()], &["password-store", "store.git"]);
}

/// Neither text holds the PIN, a name, any data that went to or from the
/// card (4 bytes or more, as hex or raw), or any of `more` and `words`.
fn assert_clean(texts: &[&str], recording: &Recording, more: &[Vec<u8>], words: &[&str]) {
    let mut secrets: Vec<Vec<u8>> = recording.data.lock().unwrap().clone();
    secrets.extend_from_slice(more);
    let pin_hex = hex(PIN.as_bytes()).0;
    for text in texts {
        for word in [PIN, SECRET, ENTRY, "github", "phone-pin", "archive", "old-forum", &pin_hex].iter().chain(words) {
            assert!(!text.contains(word), "the log shows {word:?}:\n{text}");
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

/// A rig on `recording`'s key, logging to `state`.
fn recorded_rig(state: &tempfile::TempDir, decrypt: SimTouch, kdf: bool) -> (Rig, Arc<Recording>) {
    let rig = rig_unplugged(decrypt, SimTouch::Off);
    if kdf {
        rig.sim.set_pin_kdf(3);
    }
    rig.port.set_key_log(dir_string(state), "app".into());
    let recording = Recording::new(&rig.sim);
    rig.port.plugged_in(recording.clone()).unwrap();
    (rig, recording)
}

fn all_text(state: &tempfile::TempDir, rig: &Rig) -> (String, String) {
    (key_log_text(dir_string(state), 0), rig.port.session().map(|s| s.diagnostics(0)).unwrap_or_default())
}

#[test]
fn failures_are_logged_with_no_pin_no_data_and_no_names() {
    let wrong = "918273";
    let state = tempfile::tempdir().unwrap();
    let (rig, recording) = recorded_rig(&state, SimTouch::EachUse, false);

    // A wrong PIN, then the right one.
    let driver = Driver::toucher(&rig.sim);
    let task = open(&rig, ENTRY, &driver);
    task.supply_pin(wrong.as_bytes().to_vec());
    driver.wait_for(|s| matches!(s, KeyTaskState::Pin { last_error: Some(_), .. }));
    task.supply_pin(PIN.as_bytes().to_vec());
    assert!(matches!(finish(&task), KeyTaskState::Done { .. }));

    // The key loses the PIN before a decryption: asked again.
    recording.losses.store(1, Ordering::SeqCst);
    open_and_finish(&rig, OTHER_ENTRY);

    // A touch that never comes.
    let driver = Driver::new();
    let task = open(&rig, OTHER_ENTRY, &driver);
    driver.wait_for(is_touch);
    assert!(eventually(|| rig.sim.waiting_for_touch()));
    rig.sim.time_out_touch();
    assert!(matches!(finish(&task), KeyTaskState::Failed { error: KeyError::TouchTimeout }));

    // An entry for another key.
    let task = open(&rig, FOREIGN_ENTRY, &Driver::new());
    let recipients = match finish(&task) {
        KeyTaskState::Failed { error: KeyError::NotForThisKey { recipients } } => recipients,
        other => panic!("expected NotForThisKey, got {other:?}"),
    };

    // The key loses the PIN twice in one task: it fails.
    recording.losses.store(2, Ordering::SeqCst);
    let task = open(&rig, OTHER_ENTRY, &Driver::helpful(PIN, &rig.sim));
    assert!(matches!(finish(&task), KeyTaskState::Failed { error: KeyError::Reader { .. } }));

    let (merged, ring) = all_text(&state, &rig);
    for wanted in [
        "stage Pin (again: WrongPin, 2 left)",
        "VERIFY 00 82 (with PIN): 6982 security status not satisfied",
        "PIN check 63C3: the PIN lost, ask again",
        "stage Pin (again: PinForgotten)",
        "PIN check 9000: a touch timeout",
        "stage Failed(TouchTimeout)",
        "starts at Failed(NotForThisKey)",
        "the PIN lost a second time",
        "stage Failed(Reader)",
    ] {
        assert!(merged.contains(wanted), "the log should have {wanted:?}:\n{merged}");
    }
    let mut words: Vec<&str> = vec![wrong];
    // Which keys the entry is for stays out of the log.
    let named: Vec<String> = recipients
        .iter()
        .filter_map(|recipient| match recipient {
            paw_core::recipients::Recipient::KeyId { id } => Some(id.clone()),
            paw_core::recipients::Recipient::Fingerprint { fingerprint } => Some(fingerprint.clone()),
            paw_core::recipients::Recipient::Hidden => None,
        })
        .collect();
    assert!(!named.is_empty());
    words.extend(named.iter().map(String::as_str));
    let wrong_hex = hex(wrong.as_bytes()).0;
    words.push(&wrong_hex);
    assert_clean(&[&merged, &ring], &recording, &[], &words);
}

#[test]
fn a_key_with_a_pin_kdf_is_logged_with_no_derived_pin() {
    let state = tempfile::tempdir().unwrap();
    let (rig, recording) = recorded_rig(&state, SimTouch::Off, true);
    open_and_finish(&rig, ENTRY);
    let (merged, ring) = all_text(&state, &rig);
    assert!(merged.contains("VERIFY 00 82 (with PIN): 9000 ok"), "{merged}");
    assert!(merged.contains("GET DATA 00F9: 9000 ok"), "the KDF object was read:\n{merged}");
    assert_clean(&[&merged, &ring], &recording, &[], &[]);
}

/// The log's lines without times, round trips or task numbers.
fn without_timing(state: &tempfile::TempDir) -> Vec<String> {
    key_log(dir_string(state), 0)
        .into_iter()
        .map(|line| {
            let text = line.text.split(" · ").next().unwrap().to_string();
            let mut out = String::new();
            let mut chars = text.chars().peekable();
            while let Some(c) = chars.next() {
                out.push(c);
                if c == '#' {
                    while chars.peek().is_some_and(char::is_ascii_digit) {
                        chars.next();
                    }
                }
            }
            out
        })
        .collect()
}

#[test]
fn pins_of_different_lengths_give_the_same_log() {
    let mut logs = Vec::new();
    for pin in ["735190", "73519042681"] {
        let state = tempfile::tempdir().unwrap();
        let mut rig = rig_unplugged(SimTouch::EachUse, SimTouch::Off);
        rig.sim = SimulatedKey::with_options(SimOptions {
            pin: pin.into(),
            decrypt_touch: true,
            auth_touch: false,
            touch_timeout: Duration::from_secs(5),
            extended_length: false,
        });
        rig.port.set_key_log(dir_string(&state), "app".into());
        rig.port.plugged_in(rig.sim.clone()).unwrap();
        let driver = Driver::helpful(pin, &rig.sim);
        let task = open(&rig, ENTRY, &driver);
        assert!(matches!(finish(&task), KeyTaskState::Done { .. }));
        logs.push(without_timing(&state));
    }
    assert_eq!(logs[0], logs[1]);
}

#[test]
fn the_pin_stage_and_the_verify_after_it_have_whole_second_times() {
    let state = tempfile::tempdir().unwrap();
    let rig = logged_rig(state.path(), "app", SimTouch::Off);
    let driver = Driver::new();
    let task = open(&rig, ENTRY, &driver);
    // Typing takes a while.
    std::thread::sleep(Duration::from_millis(30));
    task.supply_pin(PIN.as_bytes().to_vec());
    assert!(matches!(finish(&task), KeyTaskState::Done { .. }));

    let lines = key_log(dir_string(&state), 0);
    let start = lines.iter().position(|l| l.text.contains("starts at Pin")).unwrap();
    let verify = lines.iter().position(|l| l.text.contains("VERIFY 00 82 (with PIN)")).unwrap();
    for line in &lines[start + 1..=verify] {
        assert_eq!(line.at_ms % 1000, 0, "{line:?}");
        if line.text.starts_with('→') {
            assert!(line.text.ends_with(" s"), "{line:?}");
        }
    }
    // The VERIFY keeps its round trip in milliseconds.
    assert!(lines[verify].text.contains(" ms · +"), "{:?}", lines[verify]);
    // After it, times are exact again.
    let after = &lines[verify + 1];
    assert!(after.text.ends_with(" ms") || !after.text.starts_with('→'), "{after:?}");
    assert!(lines[verify + 1..].iter().filter(|l| l.text.starts_with('→')).all(|l| l.text.ends_with(" ms")));
}

/// A task seen in its final stage has already logged that stage: a reader
/// of the log right after the sheet closed finds how the task ended.
#[test]
fn a_task_seen_finished_has_logged_how_it_ended() {
    let rig = rig_unplugged(SimTouch::Off, SimTouch::Off);
    rig.port.plugged_in(rig.sim.clone()).unwrap();
    for _ in 0..300 {
        let driver = Driver::helpful(PIN, &rig.sim);
        let task = open(&rig, OTHER_ENTRY, &driver);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !task.state().is_final() {
            assert!(std::time::Instant::now() < deadline, "the task never finished");
            std::hint::spin_loop();
        }
        let ring = rig.port.session().unwrap().diagnostics(0);
        let started = ring.lines().rfind(|l| l.contains(" task open:")).unwrap();
        let tag = started.split_whitespace().find(|word| word.starts_with('#')).unwrap();
        assert!(ring.contains(&format!("{tag} stage Done")), "{ring}");
    }
}

#[test]
fn a_process_file_is_cut_back_to_its_last_lines() {
    let state = tempfile::tempdir().unwrap();
    let rig = logged_rig(state.path(), "app", SimTouch::Off);
    // About 15 lines an open: well past the cap.
    for _ in 0..60 {
        open_and_finish(&rig, OTHER_ENTRY);
    }
    let file = state.path().join(format!("{}app", paw_core::files::layout::KEY_LOG_PREFIX));
    let size = std::fs::metadata(&file).unwrap().len();
    assert!(size <= 41 * 1024, "{size} bytes");
    let lines = key_log(dir_string(&state), 0);
    assert!(lines.len() >= KEEP_LINES, "{} lines", lines.len());
    assert!(lines.last().unwrap().text.contains("stage Done"));
    let temp = format!(".{}app.tmp", paw_core::files::layout::KEY_LOG_PREFIX);
    let names: Vec<String> =
        std::fs::read_dir(state.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(!names.iter().any(|name| name.starts_with(&temp)), "a trim left its temporary file: {names:?}");
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
    assert!(state.path().join(format!("{}app", paw_core::files::layout::KEY_LOG_PREFIX)).exists());
    assert!(state.path().join(format!("{}fill", paw_core::files::layout::KEY_LOG_PREFIX)).exists());

    let lines = key_log(dir_string(&state), 0);
    assert!(
        lines.windows(2).all(|pair| pair[0].process == pair[1].process || pair[0].at_ms <= pair[1].at_ms),
        "merged by time"
    );
    // The processes take turns: app's first task, then fill's, then app's.
    let starts: Vec<&str> =
        lines.iter().filter(|l| l.text.contains(" task open:")).map(|l| l.process.as_str()).collect();
    assert_eq!(starts, ["app", "fill", "app", "fill", "app", "fill"]);
    // Within a process, the file's order is kept.
    let app_lines: Vec<String> =
        lines.iter().filter(|l| l.process == "app").map(|l| l.text.clone()).collect();
    let app_file = std::fs::read_to_string(state.path().join(format!("{}app", paw_core::files::layout::KEY_LOG_PREFIX))).unwrap();
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
    std::fs::write(state.path().join(paw_core::files::layout::ENROLLED_KEY), "other state").unwrap();
    // A trim the process did not live to finish.
    let trim = state.path().join(format!(".{}app.tmp-0123", paw_core::files::layout::KEY_LOG_PREFIX));
    std::fs::write(&trim, "1000\tkey plugged in\n").unwrap();

    clear_key_log(dir_string(&state)).unwrap();

    assert!(key_log(dir_string(&state), 0).is_empty());
    assert!(!trim.exists());
    assert!(state.path().join(paw_core::files::layout::ENROLLED_KEY).exists(), "only the log is cleared");
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
    assert!(state.join(format!("{}app", paw_core::files::layout::KEY_LOG_PREFIX)).exists());

    // What "Remove store from this iPhone" does: the state folder goes.
    std::fs::remove_dir_all(&state).unwrap();
    open_and_finish(&rig, ENTRY);

    assert!(!state.exists());
    assert!(key_log(state.to_string_lossy().into_owned(), 0).is_empty());
}

#[test]
fn loading_the_enrolled_key_leaves_the_log_as_it_was_set() {
    let state = tempfile::tempdir().unwrap();
    let rig = rig_unplugged(SimTouch::Off, SimTouch::Off);
    rig.port.load_enrolled_key(dir_string(&state)).unwrap();
    rig.port.plugged_in(rig.sim.clone()).unwrap();
    assert!(rig.port.key_log(0).is_empty() && key_log(dir_string(&state), 0).is_empty());

    let other = tempfile::tempdir().unwrap();
    rig.port.set_key_log(dir_string(&other), "fill".into());
    rig.port.load_enrolled_key(dir_string(&state)).unwrap();
    rig.port.plugged_in(rig.sim.clone()).unwrap();
    assert!(!rig.port.key_log(0).is_empty());
    assert!(other.path().join(format!("{}fill", paw_core::files::layout::KEY_LOG_PREFIX)).exists());
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
