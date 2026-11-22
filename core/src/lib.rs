//! paw-core: the platform-independent part of paw.
//!
//! The private key never leaves the OpenPGP card (a YubiKey). This crate
//! parses a `pass` entry, asks the card to unwrap the session key, and does
//! the symmetric decryption itself. The app supplies only a way to exchange
//! raw APDUs with the card (see [`ApduTransport`]).
//!
//! The same card signs the SSH login to the git host ([`ssh`]), which is how
//! the password store is cloned and pushed ([`sync`]).

pub mod ffi;
pub mod sim;
pub mod ssh;

uniffi::setup_scaffolding!();

use std::io::Read;
use std::sync::Arc;

use card_backend::{CardBackend, CardCaps, CardTransaction, PinType, SmartcardError};
use openpgp_card::ocard::{KeyType, StatusBytes};
use openpgp_card::state::{Open, Transaction};
use openpgp_card::Card;
use openpgp_card_rpgp::CardSlot;
use pgp::composed::Message;

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
    #[error("git error: {0}")]
    Git(String),
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
    let touch_required = matches!(
        tx.user_interaction_flag(KeyType::Decryption),
        Ok(Some(uif)) if uif.touch_policy().touch_required()
    );

    Ok(CardInfo {
        ident,
        decryption_fingerprint: fps.decryption().map(|f| f.to_string().replace(' ', "")),
        authentication_fingerprint: fps.authentication().map(|f| f.to_string().replace(' ', "")),
        pin_tries_left,
        touch_required,
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

/// Decrypts one `pass` entry (the bytes of a `.gpg` file) with the card.
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

    let mut card = open(transport)?;
    let mut tx = card.transaction().map_err(card_err)?;
    verify_pin(&mut tx, pin)?;

    let slot = CardSlot::init_from_card(&mut tx, KeyType::Decryption, on_touch_needed)
        .map_err(|e| PawError::Card(e.to_string()))?;

    let mut decrypted = slot
        .decrypt_message(message)
        .map_err(|e| PawError::Decrypt(e.to_string()))?;
    if decrypted.is_compressed() {
        decrypted = decrypted
            .decompress()
            .map_err(|e| PawError::Decrypt(e.to_string()))?;
    }

    let mut plaintext = Vec::new();
    decrypted
        .read_to_end(&mut plaintext)
        .map_err(|e| PawError::Decrypt(e.to_string()))?;
    Ok(plaintext)
}

/// Encrypts `plaintext` to the card's own decryption key, the way `pass`
/// encrypts an entry. Needs no PIN. Used to prove a key works end to end
/// without touching the password store.
pub fn encrypt_to_card(
    transport: Arc<dyn ApduTransport>,
    plaintext: &[u8],
) -> Result<Vec<u8>, PawError> {
    use pgp::composed::MessageBuilder;
    use pgp::crypto::sym::SymmetricKeyAlgorithm;

    let mut card = open(transport)?;
    let mut tx = card.transaction().map_err(card_err)?;
    let slot = CardSlot::init_from_card(&mut tx, KeyType::Decryption, &|| {})
        .map_err(|e| PawError::Card(e.to_string()))?;

    let mut rng = rand::thread_rng();
    let mut builder = MessageBuilder::from_bytes("", plaintext.to_vec())
        .seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    builder
        .encrypt_to_key(&mut rng, slot.public_key())
        .map_err(|e| PawError::Card(e.to_string()))?;
    builder
        .to_vec(&mut rng)
        .map_err(|e| PawError::Card(e.to_string()))
}
