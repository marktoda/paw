//! The key log: what paw and the card said to each other, so a report from
//! a real phone ("it said to tap my key, but…") can be read afterwards.
//!
//! Each key session keeps a ring of its last lines in memory, and each
//! process appends the same lines to its own small file in the store's
//! state directory (`key-log.app`, `key-log.fill`), so the app can show
//! what happened in the AutoFill extension too. [`key_log`] merges the
//! files by time.
//!
//! What a line can hold, and nothing else:
//!
//! - a card command's instruction byte, P1 and P2 (never its class's
//!   length byte or data), whether it carried data, its status word, its
//!   round-trip time and when it went out since the session started;
//! - a task's purpose as one word (open, setup, clone, fetch, send),
//!   whether paw held the PIN when it started, its stages, and the name of
//!   the error it ended with (never the error's text, which can name an
//!   entry or a key);
//! - the slots' touch policies, which touch watch was armed, and session
//!   events (start, end, PIN held or dropped and why).
//!
//! Never the PIN, its length or anything derived from it, entry names,
//! command or response data, or host names. A line is written after the
//! card has answered; no lock here is held across card I/O.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{KeyError, KeyPurpose, KeyTaskState, KeyTouch, LoginPurpose, apdu};
use crate::PawError;

/// Lines a session keeps in memory: about 40 card commands with the task
/// and session lines around them.
pub const RING: usize = 120;
/// A process's file is cut back to its last [`KEEP_LINES`] lines once it
/// grows past this many bytes (about 400 lines).
const TRIM_AT_BYTES: u64 = 40 * 1024;
pub const KEEP_LINES: usize = 200;
const PREFIX: &str = "key-log.";

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Plain values only.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One process's file in the state directory.
pub(crate) struct Sink {
    dir: PathBuf,
    process: String,
    /// Serialises this process's appends and trims.
    file: Mutex<()>,
}

/// A process name as it may appear in a file name: lower-case letters and
/// digits, at most 12, or "app".
fn clean_process(process: &str) -> String {
    let cleaned: String =
        process.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).take(12).collect();
    if cleaned.is_empty() { "app".into() } else { cleaned }
}

impl Sink {
    pub(crate) fn new(dir: &Path, process: &str) -> Arc<Self> {
        Arc::new(Self { dir: dir.to_path_buf(), process: clean_process(process), file: Mutex::new(()) })
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self) -> PathBuf {
        self.dir.join(format!("{PREFIX}{}", self.process))
    }

    /// Appends one line. A failure loses the line and nothing else: the log
    /// never stops a key task. Nothing is written if the state directory
    /// does not exist (before setup, or after the store was removed), so
    /// the log never makes one.
    fn append(&self, at_ms: i64, text: &str) {
        let _held = lock(&self.file);
        if !self.dir.is_dir() {
            return;
        }
        let path = self.path();
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                file.write_all(format!("{at_ms}\t{text}\n").as_bytes())?;
                file.metadata()
            });
        if written.is_ok_and(|meta| meta.len() > TRIM_AT_BYTES) {
            let _ = self.trim(&path);
        }
    }

    /// Keeps the last [`KEEP_LINES`] lines, by writing them to a new file
    /// and renaming it over the old one. The new file takes the protection
    /// class of the folder, as every state file does.
    fn trim(&self, path: &Path) -> std::io::Result<()> {
        let text = std::fs::read_to_string(path)?;
        let lines: Vec<&str> = text.lines().collect();
        let kept = &lines[lines.len().saturating_sub(KEEP_LINES)..];
        let temporary = self.dir.join(format!("{PREFIX}{}.tmp", self.process));
        std::fs::write(&temporary, kept.iter().map(|l| format!("{l}\n")).collect::<String>())?;
        std::fs::rename(&temporary, path)
    }
}

/// One key session's log.
pub(crate) struct KeyLog {
    started: Instant,
    sink: Option<Arc<Sink>>,
    ring: Mutex<VecDeque<(i64, String)>>,
    /// The PIN stage is showing: lines get whole-second times until the
    /// VERIFY with the PIN in it has been logged, so the log cannot say
    /// to the millisecond how long the PIN took to type.
    coarse: AtomicBool,
}

impl KeyLog {
    pub(crate) fn new(sink: Option<Arc<Sink>>) -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            sink,
            ring: Mutex::new(VecDeque::with_capacity(RING)),
            coarse: AtomicBool::new(false),
        })
    }

    /// The PIN stage was just shown (or a task starts at it).
    pub(crate) fn pin_asked(&self) {
        self.coarse.store(true, Ordering::SeqCst);
    }

    /// A task ended: whatever it was waiting for is over.
    pub(crate) fn task_ended(&self) {
        self.coarse.store(false, Ordering::SeqCst);
    }

    pub(crate) fn line(&self, text: impl Into<String>) {
        let text: String = text.into().replace(['\t', '\n', '\r'], " ");
        let mut at_ms = now_ms();
        if self.coarse.load(Ordering::SeqCst) {
            at_ms -= at_ms.rem_euclid(1000);
        }
        {
            let mut ring = lock(&self.ring);
            if ring.len() == RING {
                ring.pop_front();
            }
            ring.push_back((at_ms, text.clone()));
        }
        if let Some(sink) = &self.sink {
            sink.append(at_ms, &text);
        }
    }

    /// One card command, after its answer (or the reader's failure).
    /// `header` is CLA, INS, P1, P2; `has_data` whether it carried data.
    pub(crate) fn command(&self, header: [u8; 4], has_data: bool, sw: Option<u16>, round_trip: Duration, sent: Instant) {
        let since = sent.saturating_duration_since(self.started);
        let since = if self.coarse.load(Ordering::SeqCst) {
            format!("+{} s", since.as_secs())
        } else {
            format!("+{} ms", since.as_millis())
        };
        let answer = match sw {
            Some(sw) => format!("{} {}", apdu::sw_code(sw), apdu::sw_words(sw)),
            None => "reader failed".into(),
        };
        self.line(format!("→ {}: {answer} · {} ms · {since}", apdu::command_words(header, has_data), round_trip.as_millis()));
        // The PIN has gone out: times are exact again.
        if apdu::carries_pin(header, has_data) {
            self.coarse.store(false, Ordering::SeqCst);
        }
    }

    /// This session's lines, oldest first.
    pub(crate) fn text(&self, utc_offset_secs: i32) -> String {
        lock(&self.ring)
            .iter()
            .map(|(at_ms, text)| format!("{} {text}\n", clock_time(*at_ms, utc_offset_secs)))
            .collect()
    }
}

/// "12:03:07.412" at the given offset from UTC.
pub(crate) fn clock_time(at_ms: i64, utc_offset_secs: i32) -> String {
    let local = at_ms + i64::from(utc_offset_secs) * 1000;
    let in_day = local.rem_euclid(86_400_000);
    let (h, m, s, ms) = (in_day / 3_600_000, in_day / 60_000 % 60, in_day / 1000 % 60, in_day % 1000);
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

pub(crate) fn touch_words(policy: KeyTouch) -> &'static str {
    match policy {
        KeyTouch::Off => "off",
        KeyTouch::EachUse => "each use",
        KeyTouch::Cached => "cached",
        KeyTouch::Unknown => "unknown",
    }
}

/// A task's purpose in one word. Never the entry's name.
pub(crate) fn purpose_words(purpose: &KeyPurpose) -> &'static str {
    match purpose {
        KeyPurpose::OpenEntry { .. } => "open",
        KeyPurpose::CheckKey { .. } => "setup check",
        KeyPurpose::SignIn { login: LoginPurpose::Clone } => "sign in to download",
        KeyPurpose::SignIn { login: LoginPurpose::Fetch } => "sign in to check for changes",
        KeyPurpose::SignIn { login: LoginPurpose::Push { .. } } => "sign in to send",
        KeyPurpose::CheckTouchTimeout => "touch timeout check",
        KeyPurpose::Reencrypt { .. } => "re-encrypt",
    }
}

/// The error's name only: its text can name an entry or a key.
pub(crate) fn error_name(error: &KeyError) -> String {
    match error {
        KeyError::WrongPin { tries_left } => format!("WrongPin, {tries_left} left"),
        KeyError::HeldPinRejected { tries_left } => format!("HeldPinRejected, {tries_left} left"),
        KeyError::PinBlocked => "PinBlocked".into(),
        KeyError::PinFormat => "PinFormat".into(),
        KeyError::PinForgotten => "PinForgotten".into(),
        KeyError::TouchTimeout => "TouchTimeout".into(),
        KeyError::KeyRemoved => "KeyRemoved".into(),
        KeyError::NotForThisKey { .. } => "NotForThisKey".into(),
        KeyError::NoDecryptionKey => "NoDecryptionKey".into(),
        KeyError::NoAuthenticationKey => "NoAuthenticationKey".into(),
        KeyError::EntryUnreadable { .. } => "EntryUnreadable".into(),
        KeyError::DecryptFailed { .. } => "DecryptFailed".into(),
        KeyError::Reader { .. } => "Reader".into(),
        KeyError::Cancelled => "Cancelled".into(),
    }
}

/// A stage as the log shows it. `None` for progress updates, which would
/// fill the log during a download.
pub(crate) fn stage_words(state: &KeyTaskState) -> Option<String> {
    Some(match state {
        KeyTaskState::PlugIn => "PlugIn".into(),
        KeyTaskState::Pin { tries_left, forgot_after_background, last_error } => {
            let mut words = "Pin".to_string();
            if let Some(error) = last_error {
                words += &format!(" (again: {})", error_name(error));
            }
            if let Some(tries) = tries_left {
                words += &format!(" ({tries} tries left)");
            }
            if *forgot_after_background {
                words += " (forgot after background)";
            }
            words
        }
        KeyTaskState::Busy => "Busy".into(),
        KeyTaskState::Touch => "Touch".into(),
        KeyTaskState::Working { progress: None } => "Working".into(),
        KeyTaskState::Working { progress: Some(_) } => return None,
        KeyTaskState::Done { .. } => "Done".into(),
        KeyTaskState::Failed { error } => format!("Failed({})", error_name(error)),
        KeyTaskState::Cancelled => "Cancelled".into(),
    })
}

/// One line of the merged log.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeyLogLine {
    /// Milliseconds since 1970.
    pub at_ms: i64,
    /// "12:03:07.412", at the offset asked for.
    pub time: String,
    /// Which process wrote it: "app" or "fill" (the AutoFill extension).
    pub process: String,
    pub text: String,
}

impl KeyLogLine {
    pub fn render(&self) -> String {
        format!("{} {:<4} {}", self.time, self.process, self.text)
    }
}

fn log_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let process = name.strip_prefix(PREFIX)?;
            (!process.is_empty() && !process.contains('.')).then(|| (process.to_string(), entry.path()))
        })
        .collect();
    files.sort();
    files
}

/// Every process's lines in `state_dir`, merged by time, oldest first.
/// Each process's lines keep their file order: lines written while the PIN
/// stage showed carry whole-second times, so a file is not strictly in
/// time order, and the merge takes the earliest next line of any file.
#[uniffi::export]
pub fn key_log(state_dir: String, utc_offset_secs: i32) -> Vec<KeyLogLine> {
    let mut files: Vec<std::iter::Peekable<std::vec::IntoIter<KeyLogLine>>> = Vec::new();
    for (process, path) in log_files(Path::new(&state_dir)) {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let lines: Vec<KeyLogLine> = text
            .lines()
            .filter_map(|line| {
                let (at, text) = line.split_once('\t')?;
                let at_ms = at.parse::<i64>().ok()?;
                Some(KeyLogLine {
                    at_ms,
                    time: clock_time(at_ms, utc_offset_secs),
                    process: process.clone(),
                    text: text.to_string(),
                })
            })
            .collect();
        files.push(lines.into_iter().peekable());
    }
    let mut merged = Vec::new();
    loop {
        let next = files
            .iter_mut()
            .enumerate()
            .filter_map(|(index, file)| file.peek().map(|line| (line.at_ms, index)))
            .min();
        let Some((_, index)) = next else { break };
        merged.extend(files[index].next());
    }
    merged
}

/// [`key_log`] as plain text, one line each, oldest first: what Settings ›
/// Key diagnostics copies. It holds no secrets (see the module comment).
#[uniffi::export]
pub fn key_log_text(state_dir: String, utc_offset_secs: i32) -> String {
    key_log(state_dir, utc_offset_secs).iter().map(|line| line.render() + "\n").collect()
}

/// Deletes every process's key log in `state_dir`.
#[uniffi::export]
pub fn clear_key_log(state_dir: String) -> Result<(), PawError> {
    let dir = Path::new(&state_dir);
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for entry in entries.filter_map(Result::ok) {
        if entry.file_name().to_string_lossy().starts_with(PREFIX) {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(PawError::Store(format!("could not clear the key log: {e}"))),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_time_is_local() {
        // 1970-01-01 00:00:01.234 UTC, shown at UTC+1 and UTC-1.
        assert_eq!(clock_time(1234, 3600), "01:00:01.234");
        assert_eq!(clock_time(1234, -3600), "23:00:01.234");
    }

    #[test]
    fn the_ring_is_bounded() {
        let log = KeyLog::new(None);
        for n in 0..(RING * 3) {
            log.line(format!("line {n}"));
        }
        let text = log.text(0);
        assert_eq!(text.lines().count(), RING);
        assert!(text.lines().last().unwrap().ends_with(&format!("line {}", RING * 3 - 1)));
    }

    #[test]
    fn a_process_name_cannot_leave_the_folder() {
        assert_eq!(clean_process("../Fill"), "fill");
        assert_eq!(clean_process(""), "app");
    }
}
