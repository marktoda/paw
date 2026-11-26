//! The card through the openpgp-card library: what a session reads when a
//! key is plugged in (identity, slots, touch policies, public keys), and the
//! library's card handle a card job works with. The app supplies only a way
//! to exchange raw APDUs ([`CardConnection`]).

use std::sync::Arc;

use card_backend::{CardBackend, CardCaps, CardTransaction, PinType, SmartcardError};
use openpgp_card::Card;
use openpgp_card::ocard::KeyType;
use openpgp_card::ocard::algorithm::{AlgorithmAttributes, Curve};
use openpgp_card::ocard::crypto::PublicKeyMaterial;
use openpgp_card::state::{Open, Transaction};
use openpgp_card_rpgp::CardSlot;

use super::KeyError;
use crate::ssh::SshPublicKey;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ConnectionError {
    #[error("{message}")]
    Failed { message: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for ConnectionError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Failed { message: e.reason }
    }
}

/// Exchanges one command APDU for one response APDU (including the two
/// status bytes). Implemented in Swift on the USB-C smart card connection,
/// and by the simulated key.
#[uniffi::export(with_foreign)]
pub trait CardConnection: Send + Sync {
    /// Sends one command APDU and returns the response with its status bytes.
    fn transmit(&self, apdu: Vec<u8>) -> Result<Vec<u8>, ConnectionError>;
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

/// A reader that fails while the card is read fails the read; a card that
/// answers without a policy has an unknown one.
fn key_touch(tx: &mut Card<Transaction<'_>>, slot: KeyType) -> Result<KeyTouch, CardError> {
    use openpgp_card::ocard::data::TouchPolicy;
    Ok(match tx.user_interaction_flag(slot) {
        Ok(Some(uif)) => match uif.touch_policy() {
            TouchPolicy::Off => KeyTouch::Off,
            TouchPolicy::On | TouchPolicy::Fixed => KeyTouch::EachUse,
            TouchPolicy::Cached | TouchPolicy::CachedFixed => KeyTouch::Cached,
            TouchPolicy::Unknown(_) => KeyTouch::Unknown,
        },
        // Cards without the touch feature have no such data object.
        Ok(None) => KeyTouch::Off,
        Err(e @ openpgp_card::Error::Smartcard(_)) => return Err(card_err(e)),
        Err(_) => KeyTouch::Unknown,
    })
}

struct Backend(Arc<dyn CardConnection>);

struct Tx(Arc<dyn CardConnection>);

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
        self.0.transmit(cmd.to_vec()).map_err(|e| SmartcardError::Error(e.to_string()))
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

/// Why reading the card failed: the reader, or the card's answer.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CardError {
    #[error("could not talk to the key: {0}")]
    Transport(String),
    #[error("card error: {0}")]
    Card(String),
}

fn card_err(e: openpgp_card::Error) -> CardError {
    match e {
        openpgp_card::Error::Smartcard(e) => CardError::Transport(e.to_string()),
        other => CardError::Card(other.to_string()),
    }
}

/// The library's handle on the card behind `transport`; selects the
/// OpenPGP application.
pub(crate) fn open(transport: Arc<dyn CardConnection>) -> Result<Card<Open>, CardError> {
    let backend: Box<dyn CardBackend + Send + Sync> = Box::new(Backend(transport));
    Card::<Open>::new(backend).map_err(card_err)
}

/// Reads the card's identity and key state. Needs no PIN and no touch.
pub(crate) fn card_info(transport: Arc<dyn CardConnection>) -> Result<CardInfo, CardError> {
    let mut card = open(transport)?;
    let mut tx = card.transaction().map_err(card_err)?;

    let ident = tx.application_identifier().map_err(card_err)?.ident();
    let fps = tx.fingerprints().map_err(card_err)?;
    let pin_tries_left = tx.pw_status_bytes().map_err(card_err)?.err_count_pw1();
    let decryption_touch = key_touch(&mut tx, KeyType::Decryption)?;
    let authentication_touch = key_touch(&mut tx, KeyType::Authentication)?;

    Ok(CardInfo {
        ident,
        decryption_fingerprint: fps.decryption().map(|f| f.to_string().replace(' ', "")),
        authentication_fingerprint: fps.authentication().map(|f| f.to_string().replace(' ', "")),
        pin_tries_left,
        decryption_touch,
        authentication_touch,
    })
}

/// The public half of the card's decryption key. Needs no PIN.
pub(crate) fn card_decryption_key(transport: Arc<dyn CardConnection>) -> Result<pgp::packet::PublicKey, CardError> {
    let mut card = open(transport)?;
    let mut tx = card.transaction().map_err(card_err)?;
    let slot = CardSlot::init_from_card(&mut tx, KeyType::Decryption, &|| {})
        .map_err(|e| CardError::Card(e.to_string()))?;
    Ok(slot.public_key().clone())
}

/// Reads the SSH public key of the card's authentication slot. Needs no PIN
/// and no touch. The inner error says why the slot can't log in:
/// `NoAuthenticationKey`, or `AuthenticationKeyNotEd25519`.
pub(crate) fn card_ssh_public_key(
    transport: Arc<dyn CardConnection>,
) -> Result<Result<SshPublicKey, KeyError>, CardError> {
    let mut card = open(transport)?;
    let mut tx = card.transaction().map_err(card_err)?;

    if tx.fingerprint(KeyType::Authentication).map_err(card_err)?.is_none() {
        return Ok(Err(KeyError::NoAuthenticationKey));
    }
    let not_ed25519 = |algorithm: &str| Ok(Err(KeyError::AuthenticationKeyNotEd25519 { algorithm: algorithm.into() }));
    let key = match tx.public_key_material(KeyType::Authentication).map_err(card_err)? {
        PublicKeyMaterial::R(_) => return not_ed25519("RSA"),
        PublicKeyMaterial::E(key) => key,
    };
    let curve = match key.algo() {
        AlgorithmAttributes::Ecc(ecc) => ecc.curve().clone(),
        AlgorithmAttributes::Rsa(_) => return not_ed25519("RSA"),
        AlgorithmAttributes::Unknown(_) => return not_ed25519("an unknown kind of key"),
    };
    match (curve, <&[u8; 32]>::try_from(key.data())) {
        (Curve::Ed25519, Ok(public)) => Ok(Ok(SshPublicKey::ed25519(public))),
        (Curve::Ed25519, Err(_)) => not_ed25519("an Ed25519 key of the wrong length"),
        (curve, _) => not_ed25519(curve_words(&curve)),
    }
}

/// The name a person may know a curve by.
fn curve_words(curve: &Curve) -> &'static str {
    match curve {
        Curve::NistP256r1 => "ECDSA P-256",
        Curve::NistP384r1 => "ECDSA P-384",
        Curve::NistP521r1 => "ECDSA P-521",
        Curve::BrainpoolP256r1 => "Brainpool P-256",
        Curve::BrainpoolP384r1 => "Brainpool P-384",
        Curve::BrainpoolP512r1 => "Brainpool P-512",
        Curve::Secp256k1 => "secp256k1",
        Curve::Ed25519 => "Ed25519",
        Curve::Curve25519 => "Curve25519",
        Curve::Ed448 => "Ed448",
        Curve::X448 => "X448",
        Curve::Unknown(_) => "an unknown kind of key",
    }
}
