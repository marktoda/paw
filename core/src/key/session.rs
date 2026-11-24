//! One connected card: what it said about itself, the held PIN, and the
//! rule that only one card job runs at a time.
//!
//! A card job is a short run of commands with the card to itself: select
//! the application, make sure the PIN is verified, then decrypt or sign.
//! PIN entry happens between jobs, so a person typing never holds the card.
//! A task that wants the card while another job runs waits for it, showing
//! `Busy` (the queue rule); that includes a job whose task was cancelled
//! while its command waits for a touch, because nothing in Rust can
//! interrupt a command the card is holding.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::Duration;

use openpgp_card::Card;
use openpgp_card::state::Open;
use zeroize::Zeroizing;

use super::kdf::{self, PinKdf};
use super::keylog::{self, KeyLog, Sink};
use super::status::{self, Verify};
use super::task::TaskCore;
use super::wire::Wire;
use super::{KeyError, KeyTaskState, MAX_PIN_LEN, MIN_PIN_LEN, PIN_BACKGROUND_LIMIT};
use crate::copies::Clock;
use crate::ssh::SshPublicKey;
use crate::{ApduTransport, CardInfo, PawError};

/// A user PIN of a plausible length, wiped when dropped. It has no
/// `Display`, and its `Debug` shows nothing of it.
pub(crate) struct Pin(Zeroizing<Vec<u8>>);

impl Pin {
    /// Refuses a PIN the card would certainly reject, so it costs no try.
    pub(crate) fn new(bytes: Vec<u8>) -> Result<Self, KeyError> {
        let bytes = Zeroizing::new(bytes);
        if (MIN_PIN_LEN..=MAX_PIN_LEN).contains(&bytes.len()) {
            Ok(Self(bytes))
        } else {
            Err(KeyError::PinFormat)
        }
    }

    fn copy(&self) -> Self {
        Self(Zeroizing::new(self.0.to_vec()))
    }

    /// VERIFY for PW1 in mode 82 (decryption and authentication), with
    /// the PIN as the card's KDF wants it.
    fn verify_apdu(&self, kdf: &PinKdf) -> Result<Zeroizing<Vec<u8>>, KeyError> {
        let data = kdf.derive(&self.0)?;
        let mut apdu = Zeroizing::new(Vec::with_capacity(5 + data.len()));
        // The PIN's length was checked, and a KDF hash is at most 64 bytes:
        // a short APDU either way.
        apdu.extend_from_slice(&[0x00, 0x20, 0x00, 0x82, data.len() as u8]);
        apdu.extend_from_slice(&data);
        Ok(apdu)
    }
}

impl std::fmt::Debug for Pin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pin(<redacted>)")
    }
}

/// Why a card job stopped.
#[derive(Debug)]
pub(crate) enum Halt {
    /// The PIN is needed from the person; `Some` says why it is asked again.
    AskPin(Option<KeyError>),
    Fail(KeyError),
}

impl From<KeyError> for Halt {
    fn from(error: KeyError) -> Self {
        Self::Fail(error)
    }
}

/// Read once, when the key is plugged in. Needs no PIN.
struct Identity {
    info: CardInfo,
    decryption_key: Option<pgp::packet::PublicKey>,
    ssh_key: Option<SshPublicKey>,
    /// How VERIFY must carry the PIN.
    pin_kdf: PinKdf,
}

struct State {
    pin: Option<Pin>,
    /// The card's PIN counter as last seen. Only paw uses the card while it
    /// stays plugged in here, so it changes only through paw's VERIFYs.
    tries_left: u8,
    card_busy: bool,
    /// Monotonic milliseconds when paw went to the background.
    backgrounded_at_ms: Option<i64>,
    /// The PIN was dropped for the background limit and not asked since.
    forgot_after_background: bool,
    /// The PIN was dropped while a job ran: de-verify the card after it.
    deverify_pending: bool,
    /// Counts the times the PIN was dropped (Lock now, background limit,
    /// removal), so a VERIFY that was already under way when it happened
    /// does not hold the PIN again.
    forgets: u64,
    /// Tasks to wake when the session ends.
    tasks: Vec<Weak<TaskCore>>,
}

/// One connected key, from plug-in to removal.
#[derive(uniffi::Object)]
pub struct KeySession {
    /// This session, for the thread that de-verifies the card after a
    /// forget: the caller of `forget_pin` never waits for the card.
    me: Weak<KeySession>,
    wire: Arc<Wire>,
    identity: Identity,
    clock: Arc<dyn Clock>,
    ended: AtomicBool,
    state: Mutex<State>,
    card_free: Condvar,
}

/// A refusal quicker than this was not a touch timeout: a YubiKey waits
/// about 15 seconds for a touch before it gives up.
pub(crate) const FAST_REFUSAL: Duration = Duration::from_secs(1);

/// What the PIN stage shows.
pub(crate) struct PinPrompt {
    pub(crate) tries_left: u8,
    pub(crate) forgot_after_background: bool,
}

fn read_error(wire: &Wire, error: PawError) -> KeyError {
    if wire.failed() {
        KeyError::KeyRemoved
    } else {
        KeyError::Reader { detail: error.to_string() }
    }
}

impl KeySession {
    /// Reads the key's identity, key slots and touch policies. No PIN, no
    /// touch. Every command goes to a key log written to `sink`, if given;
    /// `again` says the port had read a key before.
    pub(crate) fn open(
        transport: Arc<dyn ApduTransport>,
        clock: Arc<dyn Clock>,
        sink: Option<Arc<Sink>>,
        again: bool,
    ) -> Result<Arc<Self>, KeyError> {
        let log = KeyLog::new(sink);
        log.line(if again { "session start (key read again)" } else { "session start" });
        let wire = Wire::logged(transport, log.clone());
        let opened = Self::read(wire, clock);
        match &opened {
            Ok(session) => {
                let info = session.card_info();
                log.line(format!(
                    "session ready: PIN tries left {}, touch decryption {}, authentication {}",
                    info.pin_tries_left,
                    keylog::touch_words(info.decryption_touch),
                    keylog::touch_words(info.authentication_touch),
                ));
            }
            Err(error) => log.line(format!("session failed to start: {}", keylog::error_name(error))),
        }
        opened
    }

    fn read(wire: Arc<Wire>, clock: Arc<dyn Clock>) -> Result<Arc<Self>, KeyError> {
        let info = crate::card_info(wire.clone()).map_err(|e| read_error(&wire, e))?;
        let decryption_key = match info.decryption_fingerprint {
            Some(_) => Some(crate::card_decryption_key(wire.clone()).map_err(|e| read_error(&wire, e))?),
            None => None,
        };
        let ssh_key = match crate::ssh::card_ssh_public_key(wire.clone()) {
            Ok(key) => Some(key),
            Err(PawError::NoSshKey(_)) => None,
            Err(e) => return Err(read_error(&wire, e)),
        };
        let pin_kdf = PinKdf::read(&wire)?;
        let tries_left = info.pin_tries_left;
        Ok(Arc::new_cyclic(|me| Self {
            me: me.clone(),
            wire,
            identity: Identity { info, decryption_key, ssh_key, pin_kdf },
            clock,
            ended: AtomicBool::new(false),
            state: Mutex::new(State {
                pin: None,
                tries_left,
                card_busy: false,
                backgrounded_at_ms: None,
                forgot_after_background: false,
                deverify_pending: false,
                forgets: 0,
                tasks: Vec::new(),
            }),
            card_free: Condvar::new(),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Nothing panics between writes that must go together.
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    /// The key was removed (or its reader failed): drop the PIN, and wake
    /// every task so it can fail with `KeyRemoved`.
    pub(crate) fn end(&self) {
        if !self.ended.swap(true, Ordering::SeqCst) {
            self.log().line(if self.wire.failed() { "session end (the reader failed)" } else { "session end" });
        }
        let tasks = {
            let mut state = self.lock();
            state.pin = None;
            state.forgets += 1;
            std::mem::take(&mut state.tasks)
        };
        self.card_free.notify_all();
        for task in tasks.iter().filter_map(Weak::upgrade) {
            task.wake();
        }
    }

    /// Wakes tasks waiting for the card, so a cancelled one can leave.
    pub(crate) fn wake_card_waiters(&self) {
        let _state = self.lock();
        self.card_free.notify_all();
    }

    pub(crate) fn watch(&self, task: &Arc<TaskCore>) {
        let mut state = self.lock();
        state.tasks.retain(|t| t.strong_count() > 0);
        state.tasks.push(Arc::downgrade(task));
    }

    pub(crate) fn card_info(&self) -> &CardInfo {
        &self.identity.info
    }

    pub(crate) fn decryption_key(&self) -> Result<&pgp::packet::PublicKey, KeyError> {
        self.identity.decryption_key.as_ref().ok_or(KeyError::NoDecryptionKey)
    }

    pub(crate) fn ssh_key(&self) -> Result<&SshPublicKey, KeyError> {
        self.identity.ssh_key.as_ref().ok_or(KeyError::NoAuthenticationKey)
    }

    pub(crate) fn wire(&self) -> &Arc<Wire> {
        &self.wire
    }

    pub(crate) fn log(&self) -> &Arc<KeyLog> {
        self.wire.log()
    }

    pub(crate) fn card_busy(&self) -> bool {
        self.lock().card_busy
    }

    /// Applies the background limit, then says what the PIN stage shows.
    pub(crate) fn pin_prompt(&self) -> PinPrompt {
        self.apply_background_limit();
        let state = self.lock();
        PinPrompt { tries_left: state.tries_left, forgot_after_background: state.forgot_after_background }
    }

    pub(crate) fn holds_pin_now(&self) -> bool {
        self.apply_background_limit();
        self.lock().pin.is_some()
    }

    /// If paw has been in the background for the limit or longer, the PIN
    /// is dropped, even if nobody said paw came back. A suspended process
    /// runs no code, so this is checked before anything uses the PIN.
    fn apply_background_limit(&self) {
        let now = self.clock.monotonic_ms();
        let expired = {
            let state = self.lock();
            state.backgrounded_at_ms.is_some_and(|at| {
                now.saturating_sub(at) >= PIN_BACKGROUND_LIMIT.as_millis() as i64
            }) && state.pin.is_some()
        };
        if expired {
            self.forget(true);
        }
    }

    /// Drops the PIN at once and de-verifies the card without waiting for
    /// it: on a thread of its own if the card is free, after the running
    /// job otherwise. Never sends anything on the caller's thread, which is
    /// often the app's main thread: a reader can take seconds to answer.
    fn forget(&self, after_background: bool) {
        self.log().line(if after_background { "PIN dropped (background limit)" } else { "PIN dropped (Lock now)" });
        let deverify_now = {
            let mut state = self.lock();
            state.pin = None;
            state.forgets += 1;
            state.forgot_after_background = after_background;
            if after_background {
                state.backgrounded_at_ms = None;
            }
            if self.ended() {
                false
            } else if state.card_busy {
                state.deverify_pending = true;
                false
            } else {
                // Taken now, so a task started meanwhile waits (`Busy`)
                // until the de-verify has gone out.
                state.card_busy = true;
                true
            }
        };
        if deverify_now {
            self.deverify_in_background();
        }
    }

    /// Sends the de-verify on its own thread, then frees the card. The card
    /// must already be marked busy.
    fn deverify_in_background(&self) {
        let spawned = self.me.upgrade().map(|session| {
            std::thread::Builder::new().name("paw-card-forget".into()).spawn(move || session.deverify_and_release())
        });
        if !matches!(spawned, Some(Ok(_))) {
            // No thread: the next card job de-verifies first.
            let mut state = self.lock();
            state.deverify_pending = true;
            state.card_busy = false;
            drop(state);
            self.card_free.notify_all();
        }
    }

    fn deverify_and_release(&self) {
        self.deverify();
        self.lock().card_busy = false;
        self.card_free.notify_all();
        if self.wire.failed() {
            self.end();
        }
    }

    /// VERIFY with P1 `FF`: the card forgets that the PIN was verified, so
    /// no other app can ride on it. Unverified on a YubiKey over the
    /// iPhone's reader; paw does not rely on it, because it asks for the
    /// PIN whenever it holds none, whatever the card says.
    fn deverify(&self) {
        let _ = self.wire.send(&[0x00, 0x20, 0xFF, 0x82]);
    }

    fn set_tries(&self, tries: u8) {
        self.lock().tries_left = tries;
    }

    /// Holds `pin`, unless it was dropped since `forgets` was read.
    fn hold(&self, pin: Pin, forgets: u64) {
        let held = {
            let mut state = self.lock();
            let held = state.forgets == forgets;
            if held {
                state.pin = Some(pin);
                state.forgot_after_background = false;
            }
            held
        };
        self.log().line(if held { "PIN held" } else { "PIN not held (dropped while it was checked)" });
    }

    /// Drops the held PIN; `why` is for the key log.
    fn drop_pin(&self, why: &str) {
        self.lock().pin = None;
        self.log().line(format!("PIN dropped ({why})"));
    }

    fn held_pin(&self) -> Option<Pin> {
        self.lock().pin.as_ref().map(Pin::copy)
    }

    /// What it means that the card refused PSO:DECIPHER or INTERNAL
    /// AUTHENTICATE with `sw` (`6982` or `6985`) after `elapsed`.
    /// `touch_possible` says the slot's policy can ask for a touch. Called
    /// in the same card job, right after the refusal.
    ///
    /// Both a touch that never came and a PIN verification the card lost
    /// give that answer, so the card is asked whether the PIN is still
    /// verified (an empty VERIFY, which uses no try):
    ///
    /// - not verified: the held PIN is dropped and the person is asked for
    ///   it again (`PinForgotten`, "Your key asked for its PIN again"). paw
    ///   does not re-send the held PIN here: something reset the card
    ///   mid-job, and a second loss in the same task fails instead, so this
    ///   never loops;
    /// - still verified, on a slot that can ask for a touch, and the refusal
    ///   took [`FAST_REFUSAL`] or longer: a touch timeout;
    /// - still verified but quicker than that: not a timeout (a YubiKey
    ///   waits about 15 seconds for a touch), so "Try again and touch the
    ///   gold part" would be wrong advice. It is reported as the key's own
    ///   refusal with its status word, which the key log also has. So is a
    ///   refusal on a slot that never asks for a touch.
    ///
    /// A card that cannot say whether the PIN is verified is judged on the
    /// time alone.
    pub(crate) fn refused(&self, task: &TaskCore, step: &str, sw: u16, touch_possible: bool, elapsed: Duration) -> Halt {
        let check = match self.wire.send(&[0x00, 0x20, 0x00, 0x82]) {
            Ok((_, check)) => check,
            Err(error) => return Halt::Fail(error),
        };
        let slow = elapsed >= FAST_REFUSAL;
        let (halt, reading) = match status::verify(check) {
            Verify::Verified | Verify::Other(_) if touch_possible && slow => {
                (Halt::Fail(KeyError::TouchTimeout), "a touch timeout")
            }
            Verify::Verified | Verify::Other(_) if touch_possible => (
                Halt::Fail(KeyError::Reader { detail: format!("{step}: the key refused at once ({sw:04X})") }),
                "a refusal, too quick for a touch timeout",
            ),
            Verify::Verified | Verify::Other(_) => (
                Halt::Fail(KeyError::Reader { detail: format!("{step}: the key refused ({sw:04X})") }),
                "a refusal, and the slot asks for no touch",
            ),
            Verify::NotVerified { tries_left } => {
                if let Some(tries) = tries_left {
                    self.set_tries(tries);
                }
                self.drop_pin("the key lost its verification");
                if task.note_pin_lost() > 1 {
                    (
                        Halt::Fail(KeyError::Reader {
                            detail: format!("{step}: the key lost the PIN twice ({sw:04X})"),
                        }),
                        "the PIN lost a second time",
                    )
                } else {
                    (Halt::AskPin(Some(KeyError::PinForgotten)), "the PIN lost, ask again")
                }
            }
            Verify::Blocked => {
                self.set_tries(0);
                self.drop_pin("the key is locked");
                (Halt::Fail(KeyError::PinBlocked), "the PIN is blocked")
            }
        };
        self.log().line(format!(
            "{} refused {sw:04X} after {} ms, PIN check {check:04X}: {reading}",
            task.tag(),
            elapsed.as_millis()
        ));
        halt
    }

    /// Waits until the card is free, showing `Busy` meanwhile.
    fn acquire(&self, task: &Arc<TaskCore>) -> Result<CardGuard<'_>, KeyError> {
        let mut state = self.lock();
        let mut said_busy = false;
        loop {
            if self.ended() || self.wire.failed() {
                return Err(KeyError::KeyRemoved);
            }
            if task.cancelled() {
                return Err(KeyError::Cancelled);
            }
            if !state.card_busy {
                state.card_busy = true;
                return Ok(CardGuard { session: self });
            }
            if !said_busy {
                // Not under the lock: the listener may call into the session.
                drop(state);
                task.set(KeyTaskState::Busy);
                said_busy = true;
                state = self.lock();
                continue;
            }
            state = self.card_free.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// Runs one card job with the card to itself. A failure of the reader
    /// during the job ends the session.
    pub(crate) fn run<T>(
        &self,
        task: &Arc<TaskCore>,
        job: impl FnOnce(&mut CardJob<'_>) -> Result<T, Halt>,
    ) -> Result<T, Halt> {
        self.apply_background_limit();
        let guard = match self.acquire(task) {
            Ok(guard) => guard,
            Err(error) => {
                if self.wire.failed() {
                    self.end();
                }
                return Err(Halt::Fail(error));
            }
        };
        task.set(KeyTaskState::Working { progress: None });
        let forgets = self.lock().forgets;
        let result = match crate::open(self.wire.clone()) {
            Ok(card) => job(&mut CardJob { session: self, card, forgets }),
            Err(e) => Err(Halt::Fail(read_error(&self.wire, e))),
        };
        drop(guard);
        if self.wire.failed() {
            self.end();
            return Err(Halt::Fail(KeyError::KeyRemoved));
        }
        result
    }
}

/// The card is this job's until dropped.
struct CardGuard<'a> {
    session: &'a KeySession,
}

impl Drop for CardGuard<'_> {
    fn drop(&mut self) {
        let session = self.session;
        let deverify = std::mem::take(&mut session.lock().deverify_pending);
        if deverify && !session.ended() {
            session.deverify();
        }
        session.lock().card_busy = false;
        session.card_free.notify_all();
    }
}

/// What a card job can do. The application has just been selected.
pub(crate) struct CardJob<'a> {
    session: &'a KeySession,
    pub(crate) card: Card<Open>,
    /// [`State::forgets`] when the job started.
    forgets: u64,
}

enum Verified {
    Yes,
    Wrong { tries_left: u8 },
    Blocked,
}

/// The error for a library failure during `step`: the reader's, or the
/// card's status word if it refused, or the library's own text.
pub(crate) fn card_error(wire: &Wire, step: &str, error: impl std::fmt::Display) -> KeyError {
    if wire.failed() {
        return KeyError::KeyRemoved;
    }
    match wire.last() {
        Some((_, sw)) if sw != status::OK => status::unexpected(step, sw),
        _ => KeyError::Reader { detail: format!("{step}: {error}") },
    }
}

impl<'a> CardJob<'a> {
    pub(crate) fn session(&self) -> &'a KeySession {
        self.session
    }

    /// Makes sure the PIN is verified for this job.
    ///
    /// With `supplied`, that PIN is verified and, if right, held. Without,
    /// the held PIN is used: the card is asked whether it is still verified
    /// (an empty VERIFY, which uses no try), and only if not is it verified
    /// again, silently. If the card refuses the held PIN, it is dropped at
    /// once, having used one try, and the person is asked.
    ///
    /// No VERIFY is ever sent with no tries left. A card with a PIN KDF
    /// (read when it was plugged in) is sent the derived PIN.
    pub(crate) fn unlock(&mut self, supplied: Option<Pin>) -> Result<(), Halt> {
        let session = self.session;
        // A card whose PIN KDF paw cannot follow would count any PIN as
        // wrong: refuse before asking for one.
        if let PinKdf::Unsupported(why) = &session.identity.pin_kdf {
            return Err(Halt::Fail(kdf::unsupported(why)));
        }
        if let Some(pin) = supplied {
            if session.lock().tries_left == 0 {
                return Err(Halt::Fail(KeyError::PinBlocked));
            }
            return match self.verify(&pin)? {
                Verified::Yes => {
                    session.hold(pin, self.forgets);
                    Ok(())
                }
                Verified::Wrong { tries_left } => Err(Halt::AskPin(Some(KeyError::WrongPin { tries_left }))),
                Verified::Blocked => Err(Halt::Fail(KeyError::PinBlocked)),
            };
        }

        let Some(pin) = session.held_pin() else { return Err(Halt::AskPin(None)) };
        let (_, sw) = session.wire.send(&[0x00, 0x20, 0x00, 0x82])?;
        let tries_left = match status::verify(sw) {
            Verify::Verified => return Ok(()),
            Verify::NotVerified { tries_left: Some(tries) } => tries,
            Verify::NotVerified { tries_left: None } => self.read_tries()?,
            Verify::Blocked => 0,
            // A card that cannot say: verify with the held PIN, which
            // costs no try if it is right.
            Verify::Other(_) => self.read_tries()?,
        };
        session.set_tries(tries_left);
        if tries_left == 0 {
            session.drop_pin("the key is locked");
            return Err(Halt::Fail(KeyError::PinBlocked));
        }
        match self.verify(&pin)? {
            Verified::Yes => Ok(()),
            Verified::Wrong { tries_left } => {
                session.drop_pin("the key refused it");
                Err(Halt::AskPin(Some(KeyError::HeldPinRejected { tries_left })))
            }
            Verified::Blocked => {
                session.drop_pin("the key is locked");
                Err(Halt::Fail(KeyError::PinBlocked))
            }
        }
    }

    fn verify(&mut self, pin: &Pin) -> Result<Verified, KeyError> {
        let session = self.session;
        let (_, sw) = session.wire.send(&pin.verify_apdu(&session.identity.pin_kdf)?)?;
        let verified = match status::verify(sw) {
            Verify::Verified => {
                let tries = self.read_tries()?;
                session.set_tries(tries);
                return Ok(Verified::Yes);
            }
            Verify::NotVerified { tries_left: Some(tries) } => tries,
            // The card did not say: read the counter.
            Verify::NotVerified { tries_left: None } => self.read_tries()?,
            Verify::Blocked => 0,
            Verify::Other(sw) => return Err(status::unexpected("verifying the PIN", sw)),
        };
        session.set_tries(verified);
        Ok(if verified == 0 { Verified::Blocked } else { Verified::Wrong { tries_left: verified } })
    }

    /// The PW1 retry counter, from the PW status bytes (`C4`).
    fn read_tries(&mut self) -> Result<u8, KeyError> {
        let (body, sw) = self.session.wire.send(&[0x00, 0xCA, 0x00, 0xC4, 0x00])?;
        match body.get(4) {
            Some(tries) if sw == status::OK => Ok(*tries),
            _ if sw != status::OK => Err(status::unexpected("reading the PIN counter", sw)),
            _ => Err(KeyError::Reader { detail: "reading the PIN counter: too short".into() }),
        }
    }
}

#[uniffi::export]
impl KeySession {
    /// What the key said about itself when plugged in.
    pub fn info(&self) -> CardInfo {
        self.identity.info.clone()
    }

    /// Whether the key's PIN KDF is on and whether paw can follow it, as
    /// read when it was plugged in. For "Your key": a key whose KDF paw
    /// does not support is refused there, before any PIN is asked.
    pub fn pin_kdf(&self) -> super::kdf::PinKdfStatus {
        self.identity.pin_kdf.status()
    }

    /// Whether paw can send this key its PIN: `false` for a PIN KDF paw does
    /// not support, which every task would refuse before the PIN stage.
    pub fn pin_kdf_supported(&self) -> bool {
        !matches!(self.identity.pin_kdf, PinKdf::Unsupported(_))
    }

    /// Whether paw holds the PIN, so the next task skips the PIN stage.
    /// Answered from memory: if the background limit has passed, the PIN
    /// is dropped and the card de-verified on another thread.
    pub fn holds_pin(&self) -> bool {
        self.holds_pin_now()
    }

    /// This session's key log, oldest line first, times at `utc_offset_secs`
    /// from UTC: its last card commands (instruction, P1, P2, status word,
    /// timings) and task and session events. No data, no PIN, no names.
    pub fn diagnostics(&self, utc_offset_secs: i32) -> String {
        self.log().text(utc_offset_secs)
    }

    /// The key is still plugged in and answering.
    pub fn is_connected(&self) -> bool {
        !self.ended()
    }

    /// Lock now: drops the PIN from memory before it returns, and
    /// de-verifies the card afterwards on another thread (after the running
    /// command, if one is waiting for a touch). Sends nothing to the card on
    /// the caller's thread, so it is safe on the main thread.
    pub fn forget_pin(&self) {
        self.forget(false);
    }

    /// paw went to the background. Locking the phone alone is not this.
    pub fn note_backgrounded(&self) {
        let now = self.clock.monotonic_ms();
        let mut state = self.lock();
        state.backgrounded_at_ms.get_or_insert(now);
    }

    /// paw is back. If it was away [`PIN_BACKGROUND_LIMIT`] or longer, the
    /// PIN is dropped, and the next PIN stage says why.
    pub fn note_foregrounded(&self) {
        self.apply_background_limit();
        self.lock().backgrounded_at_ms = None;
    }
}
