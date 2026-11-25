//! Reading another recipient's public certificate and choosing the key to
//! encrypt to, the way GnuPG chooses it.
//!
//! The certificate comes from the person (a file, a paste) and is not
//! trusted: every signature that matters is checked against its primary
//! key before anything in it is believed.
//!
//! Checked:
//! - every user ID, subkey binding and revocation used is cryptographically
//!   verified against the primary key; unverifiable ones are ignored;
//! - the primary key is not revoked (any verified key revocation counts,
//!   whatever its reason) and, by its newest self-signature, not expired;
//! - it has at least one user ID with a verified self-signature that is
//!   not revoked by a newer verified certification revocation;
//! - for each subkey: its newest verified binding signature is in force
//!   (not created in the future, not past its own expiry), says the key may
//!   encrypt (communications or storage), and the key has not expired by
//!   it; no verified subkey revocation; the algorithm can encrypt;
//! - of the subkeys that pass, the most recently created is chosen, as
//!   GnuPG does. Only if none pass is the primary key itself used, and only
//!   if its self-signature says it may encrypt.
//!
//! Not checked: trust (who vouches for the certificate: the person decides
//! that by importing it), third-party certifications, revocations by a
//! designated revoker, weak hash algorithms in self-signatures (SHA-1 is
//! accepted, as older GnuPG accepts it), key sizes, unknown critical
//! subpackets, and anything published after the import: a certificate is
//! read once, when it is imported, and the chosen key's expiry is the only
//! thing paw re-checks later.

use pgp::composed::{Deserializable, SignedPublicKey};
use pgp::crypto::public_key::PublicKeyAlgorithm;
use pgp::packet::{PubKeyInner, PublicKey, Signature, SignatureType};
use pgp::types::{KeyDetails, Tag};

use super::WriteError;
use crate::recipients::hex;
use crate::store::read::MAX_FILE_BYTES;

/// The key chosen for encrypting to a certificate.
#[derive(Clone)]
pub struct EncryptionChoice {
    /// The fingerprint of the chosen (sub)key, upper-case hex.
    pub fingerprint: String,
    /// When the key was created, seconds since 1970.
    pub created: u32,
    /// When it stops being usable, seconds since 1970, if ever.
    pub expires: Option<u32>,
    /// The key, as a key packet paw can encrypt to.
    pub key: PublicKey,
}

/// What paw learnt from one certificate. Public data only.
#[derive(Clone)]
pub struct CertificateInfo {
    /// The primary key's fingerprint, upper-case hex.
    pub primary: String,
    /// The fingerprints a `.gpg-id` line may name it by: the primary key's
    /// and the chosen encryption key's.
    pub fingerprints: Vec<String>,
    /// Every other subkey, with why paw does not encrypt to it.
    pub others: Vec<(String, String)>,
    /// User IDs with a verified, unrevoked self-signature.
    pub user_ids: Vec<String>,
    /// The e-mail addresses in those user IDs, lower case.
    pub emails: Vec<String>,
    /// The key to encrypt to, or why there is none.
    pub encryption: Result<EncryptionChoice, String>,
}

pub(crate) fn fingerprint_of(key: &impl KeyDetails) -> String {
    hex(key.fingerprint().as_bytes())
}

fn cert_err(message: impl Into<String>) -> WriteError {
    WriteError::Certificate { message: message.into() }
}

/// What each certificate in `bytes` offers at `now` ([`evaluate`]).
pub fn certificate_infos(bytes: &[u8], now: u32) -> Result<Vec<CertificateInfo>, WriteError> {
    read_certificates(bytes)?.iter().map(|cert| evaluate(cert, now)).collect()
}

/// Every certificate in `bytes`, armored (`gpg --export --armor`) or
/// binary (`gpg --export`).
pub fn read_certificates(bytes: &[u8]) -> Result<Vec<SignedPublicKey>, WriteError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(cert_err("the file is too large to be a public key"));
    }
    let text = bytes.trim_ascii_start();
    // A binary export starts with the first packet's tag: 5 is a secret key.
    let first_tag = text.first().filter(|b| *b & 0x80 != 0).map(|b| if b & 0x40 != 0 { b & 0x3F } else { (b >> 2) & 0x0F });
    if text.starts_with(b"-----BEGIN PGP PRIVATE KEY") || first_tag == Some(5) {
        return Err(cert_err("this is a private key; paw only needs the public key, and does not keep this"));
    }
    let parsed: Vec<Result<SignedPublicKey, pgp::errors::Error>> = if text.starts_with(b"-----BEGIN PGP") {
        match SignedPublicKey::from_armor_many(text) {
            Ok((iter, _)) => iter.collect(),
            Err(e) => return Err(cert_err(format!("this is not a public key: {e}"))),
        }
    } else {
        match SignedPublicKey::from_bytes_many(text) {
            Ok(iter) => iter.collect(),
            Err(e) => return Err(cert_err(format!("this is not a public key: {e}"))),
        }
    };
    let certificates: Vec<SignedPublicKey> = parsed
        .into_iter()
        .collect::<Result<_, _>>()
        .map_err(|e| cert_err(format!("this is not a public key: {e}")))?;
    if certificates.is_empty() {
        return Err(cert_err("there is no public key in this file"));
    }
    Ok(certificates)
}

const OLDER: &str = "it is not the key paw encrypts to: a newer encryption subkey is";

fn created(sig: &Signature) -> u32 {
    sig.created().map_or(0, |t| t.as_secs())
}

/// Whether a signature is in force at `now`: made, and not yet expired.
fn in_force(sig: &Signature, now: u32) -> bool {
    let made = created(sig);
    let ends = sig.signature_expiration_time().map(|d| d.as_secs()).filter(|d| *d != 0);
    made <= now && ends.is_none_or(|d| u64::from(made) + u64::from(d) > u64::from(now))
}

/// When a key created at `key_created` expires by `sig`, if it does.
fn key_expiry(key_created: u32, sig: &Signature) -> Option<u32> {
    sig.key_expiration_time()
        .map(|d| d.as_secs())
        .filter(|d| *d != 0)
        .map(|d| key_created.saturating_add(d))
}

fn can_encrypt(algorithm: PublicKeyAlgorithm) -> bool {
    matches!(
        algorithm,
        PublicKeyAlgorithm::RSA
            | PublicKeyAlgorithm::RSAEncrypt
            | PublicKeyAlgorithm::ECDH
            | PublicKeyAlgorithm::ElgamalEncrypt
            | PublicKeyAlgorithm::X25519
            | PublicKeyAlgorithm::X448
    )
}

/// The e-mail address in a user ID: inside `<...>` if there is such a
/// part, else the whole ID if it looks like an address. Lower case.
pub(crate) fn email_in(user_id: &str) -> Option<String> {
    let candidate = match (user_id.rfind('<'), user_id.rfind('>')) {
        (Some(open), Some(close)) if open < close => &user_id[open + 1..close],
        _ => user_id,
    };
    let candidate = candidate.trim();
    let plausible = candidate.split('@').count() == 2
        && !candidate.starts_with('@')
        && !candidate.ends_with('@')
        && !candidate.chars().any(|c| c.is_whitespace() || c.is_control() || c == ',' || c == '<' || c == '>');
    plausible.then(|| candidate.to_lowercase())
}

/// A key packet of the kind paw encrypts to, made from `key`: the same
/// public parameters and creation time, so the same fingerprint.
fn as_public_key(key: &impl KeyDetails) -> Result<PublicKey, WriteError> {
    PubKeyInner::new(key.version(), key.algorithm(), key.created_at(), None, key.public_params().clone())
        .and_then(PublicKey::from_inner)
        .map_err(|e| cert_err(format!("the encryption key cannot be used: {e}")))
}

/// Reads one certificate as of `now` (seconds since 1970). Fails only if
/// the certificate itself is unusable (revoked, expired, no valid user
/// ID); a certificate without a usable encryption key is returned with the
/// reason in `encryption`.
pub fn evaluate(cert: &SignedPublicKey, now: u32) -> Result<CertificateInfo, WriteError> {
    let primary = &cert.primary_key;
    let primary_fpr = fingerprint_of(primary);

    let revoked = cert.details.revocation_signatures.iter().any(|sig| {
        sig.typ() == Some(SignatureType::KeyRevocation) && sig.verify_key(primary).is_ok() && created(sig) <= now
    });
    if revoked {
        return Err(cert_err(format!("the key {primary_fpr} has been revoked by its owner")));
    }

    // User IDs whose newest verified signature is a certification, not a
    // revocation of it.
    let mut user_ids = Vec::new();
    let mut self_sigs: Vec<&Signature> = Vec::new();
    for user in &cert.details.users {
        let mut verified: Vec<&Signature> = user
            .signatures
            .iter()
            .filter(|sig| in_force(sig, now) && sig.verify_certification(primary, Tag::UserId, &user.id).is_ok())
            .collect();
        verified.sort_by_key(|sig| created(sig));
        let Some(newest) = verified.last() else { continue };
        if newest.typ() == Some(SignatureType::CertRevocation) {
            continue;
        }
        self_sigs.push(newest);
        if let Some(id) = user.id.as_str() {
            user_ids.push(id.to_owned());
        }
    }
    let direct: Vec<&Signature> = cert
        .details
        .direct_signatures
        .iter()
        .filter(|sig| in_force(sig, now) && sig.verify_key(primary).is_ok())
        .collect();
    if self_sigs.is_empty() {
        return Err(cert_err(format!("the key {primary_fpr} has no valid user ID")));
    }

    // The primary key's own settings: from its newest self-signature,
    // preferring the primary user ID's, as GnuPG does.
    let primary_sig = self_sigs
        .iter()
        .filter(|sig| sig.is_primary())
        .max_by_key(|sig| created(sig))
        .or_else(|| self_sigs.iter().max_by_key(|sig| created(sig)))
        .copied();
    let newest_direct = direct.iter().max_by_key(|sig| created(sig)).copied();
    let primary_created = primary.created_at().as_secs();
    let primary_expires = [primary_sig, newest_direct]
        .into_iter()
        .flatten()
        .filter_map(|sig| key_expiry(primary_created, sig))
        .min();
    if primary_expires.is_some_and(|end| end <= now) {
        return Err(cert_err(format!("the key {primary_fpr} has expired")));
    }

    let mut best: Option<EncryptionChoice> = None;
    let mut why_none = String::from("it has no key that may encrypt");
    // Every subkey that is not encrypted to, and why: a `.gpg-id` line
    // naming one is refused with the reason, never satisfied.
    let mut others: Vec<(String, String)> = Vec::new();
    for subkey in &cert.public_subkeys {
        let fingerprint = fingerprint_of(&subkey.key);
        let verified = |typ: SignatureType| {
            subkey
                .signatures
                .iter()
                .filter(move |sig| sig.typ() == Some(typ) && sig.verify_subkey_binding(primary, &subkey.key).is_ok())
        };
        let mut skip = |why: &str| others.push((fingerprint.clone(), why.to_owned()));
        if verified(SignatureType::SubkeyRevocation).any(|sig| created(sig) <= now) {
            why_none = format!("its encryption key {fingerprint} has been revoked");
            skip("it has been revoked");
            continue;
        }
        let Some(binding) = verified(SignatureType::SubkeyBinding)
            .filter(|sig| in_force(sig, now))
            .max_by_key(|sig| created(sig))
        else {
            skip("the certificate's own signature does not vouch for it");
            continue;
        };
        let flags = binding.key_flags();
        if !(flags.encrypt_comms() || flags.encrypt_storage()) || !can_encrypt(subkey.key.algorithm()) {
            skip("it is not an encryption key");
            continue;
        }
        let created_at = subkey.key.created_at().as_secs();
        if created_at > now {
            skip("it was made in the future");
            continue;
        }
        let expires = [key_expiry(created_at, binding), primary_expires].into_iter().flatten().min();
        if expires.is_some_and(|end| end <= now) {
            why_none = format!("its encryption key {fingerprint} has expired");
            skip("it has expired");
            continue;
        }
        if best.as_ref().is_none_or(|b| created_at > b.created) {
            if let Some(previous) = best.take() {
                others.push((previous.fingerprint, OLDER.into()));
            }
            best = Some(EncryptionChoice { fingerprint, created: created_at, expires, key: as_public_key(&subkey.key)? });
        } else {
            skip(OLDER);
        }
    }
    if best.is_none() {
        let own = [primary_sig, newest_direct].into_iter().flatten().max_by_key(|sig| created(sig));
        if let Some(sig) = own
            && (sig.key_flags().encrypt_comms() || sig.key_flags().encrypt_storage())
            && can_encrypt(primary.algorithm())
        {
            best = Some(EncryptionChoice {
                fingerprint: primary_fpr.clone(),
                created: primary_created,
                expires: primary_expires,
                key: as_public_key(primary)?,
            });
        }
    }
    // A line may name the certificate only by its primary key or by the key
    // that is actually encrypted to.
    let mut fingerprints = vec![primary_fpr.clone()];
    if let Some(choice) = &best
        && choice.fingerprint != primary_fpr
    {
        fingerprints.push(choice.fingerprint.clone());
    }

    let mut emails: Vec<String> = user_ids.iter().filter_map(|id| email_in(id)).collect();
    emails.dedup();
    Ok(CertificateInfo {
        primary: primary_fpr,
        fingerprints,
        others,
        user_ids,
        emails,
        encryption: best.ok_or(why_none),
    })
}
