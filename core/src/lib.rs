//! paw-core: the platform-independent part of paw.
//!
//! The private key never leaves the OpenPGP card (a YubiKey). This crate
//! parses a `pass` entry, asks the card to unwrap the session key, and does
//! the symmetric decryption itself. The app supplies only a way to exchange
//! raw APDUs with the card (see [`ApduTransport`]).
//!
//! The same card signs the SSH login to the git host ([`ssh`]), which is how
//! the password store is cloned and pushed ([`sync`]).

pub mod copies;
pub mod diag;
pub mod entry;
pub mod ffi;
pub mod path;
pub mod recipients;
pub mod sim;
pub mod ssh;
pub mod store;
pub mod sync;
pub mod setup;
pub mod key;

uniffi::setup_scaffolding!();

use std::io::Read;
use std::sync::Arc;

use card_backend::{CardBackend, CardCaps, CardTransaction, PinType, SmartcardError};
use openpgp_card::ocard::{KeyType, StatusBytes};
use openpgp_card::state::{Open, Transaction};
use openpgp_card::Card;
use openpgp_card_rpgp::CardSlot;
use pgp::composed::{Esk, Message};
use pgp::packet::PublicKeyEncryptedSessionKey;
use pgp::types::{DecryptionKey, EskType, Password};

/// Exchanges one command APDU for one response APDU (including the two
/// status bytes). On iOS this is backed by the USB-C smart card connection;
/// in tests by a simulated card.
pub trait ApduTransport: Send + Sync {
    fn transmit(&self, apdu: &[u8]) -> Result<Vec<u8>, String>;
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum PawError {
    #[error("could not talk to the key: {0}")]
    Transport(String),
    #[error("wrong PIN, {tries_left} tries left")]
    WrongPin { tries_left: u8 },
    #[error("the PIN is blocked; unblock it with the admin PIN")]
    PinBlocked,
    #[error("this is not an encrypted pass entry: {0}")]
    NotAnEntry(String),
    #[error("the key could not decrypt this entry: {0}")]
    Decrypt(String),
    #[error("card error: {0}")]
    Card(String),
    #[error("this key cannot log in to a git host: {0}")]
    NoSshKey(String),
    #[error("the key could not sign the login: {0}")]
    Sign(String),
    #[error("could not reach the git host: {0}")]
    Network(String),
    /// The host presented a key other than the pinned one. `presented` is
    /// its SHA-256 fingerprint, for the "host key changed" screen.
    #[error("the git host's key does not match the saved one (it presented {presented})")]
    HostKeyMismatch { presented: String },
    #[error("the git host did not accept this key")]
    KeyRejected,
    /// The remote branch has commits this copy does not build on. Nothing
    /// was pushed; paw never forces.
    #[error("the remote has changes that are not in this copy")]
    NotFastForward,
    /// A file name that could reach outside the store or into git's own
    /// files. Nothing was written.
    #[error("not a safe name for an entry: {0}")]
    UnsafePath(String),
    #[error("git error: {0}")]
    Git(String),
    /// The store on this device could not be read, or holds no such thing.
    #[error("{0}")]
    Store(String),
    /// The entry decrypted, but to something other than text. It is left
    /// exactly as it is.
    #[error("this entry is not text, so paw cannot show or edit it")]
    NotText,
    /// Refused before the PIN or a touch was asked for: none of the keys the
    /// entry is encrypted for is the one plugged in. `recipients` are the
    /// keys that can open it, as people write them ("key ID 0123...").
    #[error("this entry is encrypted for another key ({})", .recipients.join(", "))]
    NotForThisKey { recipients: Vec<String> },
    /// A value that cannot be stored in an entry as given.
    #[error("{0}")]
    InvalidField(String),
    /// The entry has no live Face ID copy (none was kept, it ended, or the
    /// entry changed since). Open it with the key.
    #[error("this entry needs your key")]
    CopyUnavailable,
    /// The person closed the Face ID prompt. The copy is still there.
    #[error("Face ID was cancelled")]
    FaceIdCancelled,
    #[error("Face ID didn't recognise you")]
    FaceIdNotRecognised,
    #[error("Face ID is turned off until you enter your iPhone passcode")]
    FaceIdLockedOut,
    #[error("Face ID isn't set up on this iPhone, so saved copies can't be opened")]
    FaceIdNotSetUp,
    /// The Face ID key was invalidated (enrolment changed or the passcode
    /// was removed). Every copy has been wiped.
    #[error("Face ID changed on this iPhone, so opened passwords were cleared")]
    FaceIdChanged,
    /// The copies folder or the keystore failed in some other way. Never
    /// carries entry names or contents.
    #[error("Face ID copies: {0}")]
    CopyFailed(String),
}

/// What the card says about itself, read without a PIN.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CardInfo {
    /// Manufacturer and serial, e.g. "0006:12345678".
    pub ident: String,
    /// Hex fingerprint of the decryption key slot, if a key is loaded.
    pub decryption_fingerprint: Option<String>,
    /// Hex fingerprint of the authentication key slot (used for SSH).
    pub authentication_fingerprint: Option<String>,
    /// Remaining user PIN attempts before the PIN blocks.
    pub pin_tries_left: u8,
    /// Whether the decryption key needs a physical touch for each use.
    pub touch_required: bool,
    /// When the decryption key asks for a touch.
    pub decryption_touch: KeyTouch,
    /// When the authentication key (SSH) asks for a touch.
    pub authentication_touch: KeyTouch,
}

/// When a key slot asks for a physical touch. The app shows its touch prompt
/// from this, so a cached policy must not be mistaken for "every time".
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum KeyTouch {
    /// Never.
    Off,
    /// For every use.
    EachUse,
    /// For a use more than 15 seconds after the last touch.
    Cached,
    /// The key did not say, or said something this version does not know.
    Unknown,
}

fn key_touch(tx: &mut Card<Transaction<'_>>, slot: KeyType) -> KeyTouch {
    use openpgp_card::ocard::data::TouchPolicy;
    match tx.user_interaction_flag(slot) {
        Ok(Some(uif)) => match uif.touch_policy() {
            TouchPolicy::Off => KeyTouch::Off,
            TouchPolicy::On | TouchPolicy::Fixed => KeyTouch::EachUse,
            TouchPolicy::Cached | TouchPolicy::CachedFixed => KeyTouch::Cached,
            TouchPolicy::Unknown(_) => KeyTouch::Unknown,
        },
        // Cards without the touch feature have no such data object.
        Ok(None) => KeyTouch::Off,
        Err(_) => KeyTouch::Unknown,
    }
}

struct Backend(Arc<dyn ApduTransport>);

struct Tx(Arc<dyn ApduTransport>);

impl CardBackend for Backend {
    fn limit_card_caps(&self, card_caps: CardCaps) -> CardCaps {
        card_caps
    }

    fn transaction(
        &mut self,
        _reselect_application: Option<&[u8]>,
    ) -> Result<Box<dyn CardTransaction + Send + Sync + '_>, SmartcardError> {
        Ok(Box::new(Tx(self.0.clone())))
    }
}

impl CardTransaction for Tx {
    fn transmit(&mut self, cmd: &[u8], _buf_size: usize) -> Result<Vec<u8>, SmartcardError> {
        self.0.transmit(cmd).map_err(SmartcardError::Error)
    }

    fn feature_pinpad_verify(&self) -> bool {
        false
    }

    fn feature_pinpad_modify(&self) -> bool {
        false
    }

    fn pinpad_verify(
        &mut self,
        _pin: PinType,
        _card_caps: &Option<CardCaps>,
    ) -> Result<Vec<u8>, SmartcardError> {
        Err(SmartcardError::Error("no PIN pad".into()))
    }

    fn pinpad_modify(
        &mut self,
        _pin: PinType,
        _card_caps: &Option<CardCaps>,
    ) -> Result<Vec<u8>, SmartcardError> {
        Err(SmartcardError::Error("no PIN pad".into()))
    }

    fn was_reset(&self) -> bool {
        false
    }
}

fn card_err(e: openpgp_card::Error) -> PawError {
    match e {
        openpgp_card::Error::Smartcard(e) => PawError::Transport(e.to_string()),
        other => PawError::Card(other.to_string()),
    }
}

fn open(transport: Arc<dyn ApduTransport>) -> Result<Card<Open>, PawError> {
    let backend: Box<dyn CardBackend + Send + Sync> = Box::new(Backend(transport));
    Card::<Open>::new(backend).map_err(card_err)
}

/// Reads the card's identity and key state. Needs no PIN and no touch.
pub fn card_info(transport: Arc<dyn ApduTransport>) -> Result<CardInfo, PawError> {
    let mut card = open(transport)?;
    let mut tx = card.transaction().map_err(card_err)?;

    let ident = tx.application_identifier().map_err(card_err)?.ident();
    let fps = tx.fingerprints().map_err(card_err)?;
    let pin_tries_left = tx.pw_status_bytes().map_err(card_err)?.err_count_pw1();
    let decryption_touch = key_touch(&mut tx, KeyType::Decryption);
    let authentication_touch = key_touch(&mut tx, KeyType::Authentication);
    let touch_required = decryption_touch != KeyTouch::Off;

    Ok(CardInfo {
        ident,
        decryption_fingerprint: fps.decryption().map(|f| f.to_string().replace(' ', "")),
        authentication_fingerprint: fps.authentication().map(|f| f.to_string().replace(' ', "")),
        pin_tries_left,
        touch_required,
        decryption_touch,
        authentication_touch,
    })
}

/// Verifies the user PIN in the mode that unlocks both decryption and
/// authentication (PW1 mode 82), turning the card's refusals into
/// [`PawError::WrongPin`] and [`PawError::PinBlocked`].
fn verify_pin(tx: &mut Card<Transaction<'_>>, pin: &str) -> Result<(), PawError> {
    // The library's copy of the PIN is a `SecretString`, wiped when dropped.
    let Err(e) = tx.verify_user_pin(pin.to_owned().into()) else {
        return Ok(());
    };
    Err(match e {
        openpgp_card::Error::CardStatus(
            StatusBytes::SecurityStatusNotSatisfied | StatusBytes::PasswordNotChecked(_),
        ) => {
            // The library caches card status; a failed VERIFY changes the
            // counter on the card, so read it again.
            tx.invalidate_cache().map_err(card_err)?;
            let tries_left = tx.pw_status_bytes().map_err(card_err)?.err_count_pw1();
            if tries_left == 0 {
                PawError::PinBlocked
            } else {
                PawError::WrongPin { tries_left }
            }
        }
        openpgp_card::Error::CardStatus(StatusBytes::AuthenticationMethodBlocked) => {
            PawError::PinBlocked
        }
        other => card_err(other),
    })
}

/// Whether a recipient packet hides whom it is for (`gpg --throw-keyids`).
fn is_hidden(pkesk: &PublicKeyEncryptedSessionKey) -> bool {
    match pkesk {
        PublicKeyEncryptedSessionKey::V3 { id, .. } => id.is_wildcard(),
        PublicKeyEncryptedSessionKey::V6 { fingerprint, .. } => fingerprint.is_none(),
        _ => false,
    }
}

/// Decrypts one `pass` entry (the bytes of a `.gpg` file) with the card.
///
/// An entry can be encrypted for several keys. The card is asked to unwrap
/// only the session key addressed to it, because every request costs a
/// touch; if the entry hides its recipients, each hidden one is tried in
/// turn. An entry that names other keys only fails with
/// [`PawError::NotForThisKey`] before the PIN is sent, so it costs neither a
/// PIN try nor a touch.
///
/// `on_touch_needed` is called just before the card starts waiting for a
/// touch, so the app can show its "touch your key" prompt.
pub fn decrypt_entry(
    transport: Arc<dyn ApduTransport>,
    pin: &str,
    ciphertext: &[u8],
    on_touch_needed: &(dyn Fn() + Send + Sync),
) -> Result<Vec<u8>, PawError> {
    let message = Message::from_bytes(ciphertext).map_err(|e| PawError::NotAnEntry(e.to_string()))?;
    let Message::Encrypted { esk, mut edata, .. } = message else {
        return Err(PawError::NotAnEntry("it is not encrypted".into()));
    };

    let mut card = open(transport)?;
    let mut tx = card.transaction().map_err(card_err)?;
    let key = CardSlot::init_from_card(&mut tx, KeyType::Decryption, &|| {})
        .map_err(|e| PawError::Card(e.to_string()))?
        .public_key()
        .clone();

    let for_this_key: Vec<&PublicKeyEncryptedSessionKey> = esk
        .iter()
        .filter_map(|esk| match esk {
            Esk::PublicKeyEncryptedSessionKey(pkesk) if pkesk.match_identity(&key) => Some(pkesk),
            _ => None,
        })
        .collect();
    // The packets that name this key; only if there are none, the hidden ones.
    let (hidden, named): (Vec<_>, Vec<_>) = for_this_key.into_iter().partition(|p| is_hidden(p));
    let candidates = if named.is_empty() { hidden } else { named };
    if candidates.is_empty() {
        let recipients = recipients::entry_recipients(ciphertext)
            .map(|found| found.keys.iter().map(recipients::Recipient::describe).collect())
            .unwrap_or_default();
        return Err(PawError::NotForThisKey { recipients });
    }

    verify_pin(&mut tx, pin)?;
    let slot = CardSlot::with_public_key(&mut tx, KeyType::Decryption, key, on_touch_needed)
        .map_err(|e| PawError::Card(e.to_string()))?;

    let mut failure = PawError::Decrypt("the key did not answer".into());
    let mut session_key = None;
    for pkesk in candidates {
        let version = match pkesk {
            PublicKeyEncryptedSessionKey::V6 { .. } => EskType::V6,
            _ => EskType::V3_4,
        };
        let unwrapped = pkesk
            .values()
            .and_then(|values| slot.decrypt(&Password::empty(), values, version))
            .and_then(|inner| inner);
        match unwrapped {
            Ok(key) => {
                session_key = Some(key);
                break;
            }
            Err(e) => failure = PawError::Decrypt(e.to_string()),
        }
    }
    let Some(session_key) = session_key else {
        return Err(failure);
    };

    edata
        .decrypt(&session_key)
        .map_err(|e| PawError::Decrypt(e.to_string()))?;
    let mut decrypted =
        Message::from_bytes(edata).map_err(|e| PawError::Decrypt(e.to_string()))?;
    if decrypted.is_compressed() {
        decrypted = decrypted
            .decompress()
            .map_err(|e| PawError::Decrypt(e.to_string()))?;
    }

    let mut plaintext = Vec::new();
    read_capped(&mut decrypted, &mut plaintext).map_err(PawError::Decrypt)?;
    Ok(plaintext)
}

/// The most decrypted text an entry may hold. Entries are a few lines; a
/// tiny compressed file that expands without end would otherwise exhaust
/// memory, which the AutoFill extension has very little of.
pub(crate) const MAX_PLAINTEXT_BYTES: u64 = 1 << 20;

/// Reads decrypted (and possibly decompressed) text into `out`, refusing
/// anything longer than [`MAX_PLAINTEXT_BYTES`].
pub(crate) fn read_capped(from: &mut impl Read, out: &mut Vec<u8>) -> Result<(), String> {
    Read::take(from, MAX_PLAINTEXT_BYTES + 1)
        .read_to_end(out)
        .map_err(|e| e.to_string())?;
    if out.len() as u64 > MAX_PLAINTEXT_BYTES {
        use zeroize::Zeroize;
        out.zeroize();
        return Err("it holds more text than any password entry should".into());
    }
    Ok(())
}

/// The public half of the card's decryption key. Needs no PIN.
pub fn card_decryption_key(
    transport: Arc<dyn ApduTransport>,
) -> Result<pgp::packet::PublicKey, PawError> {
    let mut card = open(transport)?;
    let mut tx = card.transaction().map_err(card_err)?;
    let slot = CardSlot::init_from_card(&mut tx, KeyType::Decryption, &|| {})
        .map_err(|e| PawError::Card(e.to_string()))?;
    Ok(slot.public_key().clone())
}

/// Encrypts `plaintext` to every key in `keys`, the way `pass` encrypts an
/// entry (`gpg -e -r ... -r ...`). Recipients are a list everywhere in the
/// core, even while the app only ever passes one.
pub fn encrypt_to_keys<R: rand::Rng + rand::CryptoRng>(
    mut rng: R,
    keys: &[pgp::packet::PublicKey],
    plaintext: &[u8],
) -> Result<Vec<u8>, PawError> {
    use pgp::composed::MessageBuilder;
    use pgp::crypto::sym::SymmetricKeyAlgorithm;

    if keys.is_empty() {
        return Err(PawError::Card("there is no key to encrypt to".into()));
    }
    let mut builder = MessageBuilder::from_bytes("", plaintext.to_vec())
        .seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    for key in keys {
        builder
            .encrypt_to_key(&mut rng, key)
            .map_err(|e| PawError::Card(e.to_string()))?;
    }
    builder
        .to_vec(&mut rng)
        .map_err(|e| PawError::Card(e.to_string()))
}

/// Encrypts `plaintext` to the card's own decryption key, the way `pass`
/// encrypts an entry. Needs no PIN. Used to prove a key works end to end
/// without touching the password store.
pub fn encrypt_to_card(
    transport: Arc<dyn ApduTransport>,
    plaintext: &[u8],
) -> Result<Vec<u8>, PawError> {
    let key = card_decryption_key(transport)?;
    encrypt_to_keys(rand::thread_rng(), &[key], plaintext)
}
