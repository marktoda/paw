//! SSH login with the card's authentication key.
//!
//! An SSH client proves who it is by signing one block of data per
//! connection. Here the card makes that signature, so the SSH private key
//! never exists outside it (`key::SessionSigner`). The public half is read
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
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>, crate::key::KeyError>;
}
