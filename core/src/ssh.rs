//! SSH login with the card's authentication key.
//!
//! An SSH client proves who it is by signing one block of data per
//! connection. Here the card makes that signature, so the SSH private key
//! never exists outside it. The public half is read without a PIN, which lets
//! the app show the `authorized_keys` line during setup before asking for
//! anything.
//!
//! Only ed25519 is supported. The card signs the data as given (PureEdDSA),
//! which is exactly what `ssh-ed25519` wants, so no hashing happens here.

use std::sync::Arc;

use base64::Engine;
use openpgp_card::ocard::KeyType;
use openpgp_card::ocard::algorithm::{AlgorithmAttributes, Curve};
use openpgp_card::ocard::crypto::PublicKeyMaterial;
use zeroize::Zeroizing;

use crate::{ApduTransport, PawError, card_err, open, verify_pin};

const ED25519: &str = "ssh-ed25519";

/// An SSH public key in its wire encoding (RFC 4253, 6.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshPublicKey {
    blob: Vec<u8>,
}

impl SshPublicKey {
    pub fn ed25519(public: &[u8; 32]) -> Self {
        let mut blob = Vec::with_capacity(51);
        for field in [ED25519.as_bytes(), public] {
            blob.extend((field.len() as u32).to_be_bytes());
            blob.extend(field);
        }
        Self { blob }
    }

    /// The algorithm name, which also names the signature format.
    pub fn algorithm(&self) -> &'static str {
        ED25519
    }

    /// The key as the SSH protocol carries it.
    pub fn blob(&self) -> &[u8] {
        &self.blob
    }

    /// The line to paste into `authorized_keys` or a git host's key page.
    pub fn authorized_key(&self) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&self.blob);
        format!("{} {encoded}", self.algorithm())
    }
}

/// Something that can sign an SSH login: the card today, a device key later.
pub trait SshSigner: Send + Sync {
    fn public_key(&self) -> &SshPublicKey;

    /// Signs `data` and returns the bare signature (64 bytes for ed25519).
    /// May block for as long as the key waits for a touch.
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, PawError>;
}

/// Reads the SSH public key of the card's authentication slot. Needs no PIN
/// and no touch.
pub fn card_ssh_public_key(transport: Arc<dyn ApduTransport>) -> Result<SshPublicKey, PawError> {
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

/// Signs `data` with the card's authentication key and returns the 64-byte
/// ed25519 signature.
///
/// `on_touch_needed` is called just before the card starts waiting for a
/// touch, if the authentication slot's own touch policy asks for one.
pub fn card_ssh_sign(
    transport: Arc<dyn ApduTransport>,
    pin: &str,
    data: &[u8],
    on_touch_needed: &(dyn Fn() + Send + Sync),
) -> Result<Vec<u8>, PawError> {
    let mut card = open(transport)?;
    let mut tx = card.transaction().map_err(card_err)?;
    verify_pin(&mut tx, pin)?;

    if matches!(
        tx.user_interaction_flag(KeyType::Authentication),
        Ok(Some(uif)) if uif.touch_policy().touch_required()
    ) {
        on_touch_needed();
    }
    let signature = tx
        .card()
        .internal_authenticate(data.to_vec())
        .map_err(|e| PawError::Sign(e.to_string()))?;
    if signature.len() != 64 {
        return Err(PawError::Sign(format!(
            "expected a 64-byte signature, the key returned {} bytes",
            signature.len()
        )));
    }
    Ok(signature)
}

/// The card as an [`SshSigner`], for one connection attempt.
///
/// It holds the PIN, so make it just before connecting and drop it as soon as
/// the connection is open; the PIN is wiped from memory on drop.
pub struct CardSigner {
    transport: Arc<dyn ApduTransport>,
    pin: Zeroizing<String>,
    public_key: SshPublicKey,
    on_touch_needed: Box<dyn Fn() + Send + Sync>,
}

impl CardSigner {
    /// Reads the public key from the card. The PIN is not sent until the
    /// git host has said it accepts that key and asks for the signature.
    pub fn new(
        transport: Arc<dyn ApduTransport>,
        pin: &str,
        on_touch_needed: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, PawError> {
        Ok(Self {
            public_key: card_ssh_public_key(transport.clone())?,
            transport,
            pin: Zeroizing::new(pin.to_owned()),
            on_touch_needed: Box::new(on_touch_needed),
        })
    }
}

impl SshSigner for CardSigner {
    fn public_key(&self) -> &SshPublicKey {
        &self.public_key
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, PawError> {
        card_ssh_sign(self.transport.clone(), &self.pin, data, &*self.on_touch_needed)
    }
}
