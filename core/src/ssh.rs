//! SSH login with the card's authentication key.
//!
//! An SSH client proves who it is by signing one block of data per
//! connection. Here the card makes that signature, so the SSH private key
//! never exists outside it (`key::SshSigner`, `key::SessionSigner`). The public half is read
//! without a PIN, which lets the app show the `authorized_keys` line during
//! setup before asking for anything.
//!
//! Only ed25519 is supported. The card signs the data as given (PureEdDSA),
//! which is exactly what `ssh-ed25519` wants, so no hashing happens here.

use base64::Engine;

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

    pub fn blob(&self) -> &[u8] {
        &self.blob
    }

    /// The line to paste into `authorized_keys` or a git host's key page.
    pub fn authorized_key(&self) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&self.blob);
        format!("{} {encoded}", self.algorithm())
    }

    /// [`Self::authorized_key`] read back; `None` for anything else.
    pub(crate) fn from_authorized_key(line: &str) -> Option<Self> {
        let blob = base64::engine::general_purpose::STANDARD.decode(line.split(' ').nth(1)?).ok()?;
        // An ssh-ed25519 blob ends in the 32-byte key.
        let raw: [u8; 32] = blob.get(blob.len().checked_sub(32)?..)?.try_into().ok()?;
        let key = Self::ed25519(&raw);
        (key.blob == blob).then_some(key)
    }
}
