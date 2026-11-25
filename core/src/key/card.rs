//! The card through the openpgp-card library: what a session reads when a
//! key is plugged in (identity, slots, touch policies, public keys), and the
//! library's card handle a card job works with. The app supplies only a way
//! to exchange raw APDUs ([`ApduTransport`]).

use std::sync::Arc;

use card_backend::{CardBackend, CardCaps, CardTransaction, PinType, SmartcardError};
use openpgp_card::Card;
use openpgp_card::ocard::KeyType;
use openpgp_card::ocard::algorithm::{AlgorithmAttributes, Curve};
use openpgp_card::ocard::crypto::PublicKeyMaterial;
use openpgp_card::state::{Open, Transaction};
use openpgp_card_rpgp::CardSlot;

use crate::PawError;
use crate::ssh::SshPublicKey;

/// Exchanges one command APDU for one response APDU (including the two
/// status bytes). On iOS this is backed by the USB-C smart card connection;
/// in tests by a simulated card.
pub trait ApduTransport: Send + Sync {
    fn transmit(&self, apdu: &[u8]) -> Result<Vec<u8>, String>;
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

/// The library's handle on the card behind `transport`; selects the
/// OpenPGP application.
pub(crate) fn open(transport: Arc<dyn ApduTransport>) -> Result<Card<Open>, PawError> {
    let backend: Box<dyn CardBackend + Send + Sync> = Box::new(Backend(transport));
    Card::<Open>::new(backend).map_err(card_err)
}

/// Reads the card's identity and key state. Needs no PIN and no touch.
pub(crate) fn card_info(transport: Arc<dyn ApduTransport>) -> Result<CardInfo, PawError> {
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

/// The public half of the card's decryption key. Needs no PIN.
pub(crate) fn card_decryption_key(transport: Arc<dyn ApduTransport>) -> Result<pgp::packet::PublicKey, PawError> {
    let mut card = open(transport)?;
    let mut tx = card.transaction().map_err(card_err)?;
    let slot = CardSlot::init_from_card(&mut tx, KeyType::Decryption, &|| {})
        .map_err(|e| PawError::Card(e.to_string()))?;
    Ok(slot.public_key().clone())
}

/// Reads the SSH public key of the card's authentication slot. Needs no PIN
/// and no touch.
pub(crate) fn card_ssh_public_key(transport: Arc<dyn ApduTransport>) -> Result<SshPublicKey, PawError> {
    let mut card = open(transport)?;
    let mut tx = card.transaction().map_err(card_err)?;

    if tx.fingerprint(KeyType::Authentication).map_err(card_err)?.is_none() {
        return Err(PawError::NoSshKey("it has no authentication key".into()));
    }
    let PublicKeyMaterial::E(key) = tx
        .public_key_material(KeyType::Authentication)
        .map_err(card_err)?
    else {
        return Err(PawError::NoSshKey("its authentication key is RSA, not ed25519".into()));
    };
    let is_ed25519 =
        matches!(key.algo(), AlgorithmAttributes::Ecc(ecc) if *ecc.curve() == Curve::Ed25519);
    match <&[u8; 32]>::try_from(key.data()) {
        Ok(public) if is_ed25519 => Ok(SshPublicKey::ed25519(public)),
        _ => Err(PawError::NoSshKey("its authentication key is not ed25519".into())),
    }
}
