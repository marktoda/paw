//! The bytes of one Face ID copy on disk.
//!
//! ```text
//! "PAWCOPY1"                     8 bytes, format and version
//! meta nonce                     24
//! meta length                    u32, big-endian
//! meta ciphertext                XChaCha20-Poly1305, metadata key
//!                                AD = magic || SHA-256(everything after it)
//! wrapped data key length        u32
//! wrapped data key               DeviceKeystore::seal of the data key
//! body nonce                     24
//! body ciphertext                XChaCha20-Poly1305, data key
//!                                AD = magic || encoded metadata
//! ```
//!
//! The file is named after a keyed hash of the entry path (`<32 hex>.copy`).
//!
//! Two keys, for two jobs:
//! - The **data key** is random per copy and reaches Rust again only
//!   through Face ID. It seals the text, with the metadata (entry path,
//!   content id, times, boot id) as associated data: a text can never be
//!   opened as another entry's, or with a deadline it was not sealed with.
//! - The **metadata key** comes from the keystore's index key, usable
//!   without a prompt. It lets the core read and check every deadline
//!   before any Face ID prompt, and keeps the metadata unreadable and
//!   unforgeable to someone who has only the files. Its associated data
//!   covers the wrapped key and body, so a spliced file fails here, before
//!   a prompt.
//!
//! Both plaintexts are padded to a bucket size, so file sizes say little
//! about the path's or the text's length.

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const MAGIC: &[u8; 8] = b"PAWCOPY1";
pub const SUFFIX: &str = ".copy";
/// No copy file is larger. Entries are at most 1 MiB (`store::read`).
pub const MAX_FILE_BYTES: u64 = 2 << 20;

const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const KEY_LEN: usize = 32;
const META_BUCKET: usize = 512;
const BODY_BUCKET: usize = 256;
const MAX_ENTRY_BYTES: usize = 4096;
const MAX_ID_BYTES: usize = 255;
const MAX_WRAPPED_BYTES: usize = 4096;
/// A copy holds an entry's text, so no more than an entry may hold.
const MAX_TEXT_BYTES: usize = crate::entry::MAX_PLAINTEXT_BYTES as usize;

/// Something is wrong with a copy: it was not written by paw with these
/// keys, or it was changed since. Deliberately says nothing more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid;

/// The two keys derived from the keystore's index key.
pub struct IndexKeys {
    name: Zeroizing<[u8; KEY_LEN]>,
    meta: Zeroizing<[u8; KEY_LEN]>,
}

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], parts: &[&[u8]]) -> Zeroizing<[u8; 32]> {
    // HMAC accepts a key of any length, so this cannot fail.
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).unwrap_or_else(|_| unreachable!());
    for part in parts {
        mac.update(part);
    }
    Zeroizing::new(mac.finalize().into_bytes().into())
}

impl IndexKeys {
    /// Derives the file-name key and the metadata key. The index key must
    /// be 32 bytes; anything else is a keystore bug, refused.
    pub fn derive(index_key: &[u8]) -> Result<Self, Invalid> {
        if index_key.len() != KEY_LEN {
            return Err(Invalid);
        }
        Ok(Self {
            name: hmac(index_key, &[b"paw copy file name"]),
            meta: hmac(index_key, &[b"paw copy metadata"]),
        })
    }

    /// The file name (without [`SUFFIX`]) of `entry`'s copy: 128 bits of
    /// HMAC, so it can be found without a prompt and without a name list,
    /// and tells someone without the index key nothing about the entry.
    pub fn file_stem(&self, entry: &str) -> String {
        hmac(&*self.name, &[entry.as_bytes()])[..16].iter().map(|b| format!("{b:02x}")).collect()
    }

    fn meta_cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&*self.meta).into())
    }
}

/// What a copy says about itself. All of it is authenticated twice: by the
/// metadata key, and as associated data of the text.
#[derive(Clone, PartialEq, Eq)]
pub struct Meta {
    /// The entry's path in the store, e.g. `web/github.com`.
    pub entry: String,
    /// Identifies the ciphertext the text came from (`content_id`).
    pub content_id: Vec<u8>,
    pub created_wall_ms: i64,
    pub created_monotonic_ms: i64,
    pub deadline_wall_ms: i64,
    pub boot_id: String,
}

fn put_field(out: &mut Vec<u8>, bytes: &[u8], two_byte_len: bool) {
    if two_byte_len {
        out.extend((bytes.len() as u16).to_be_bytes());
    } else {
        out.push(bytes.len() as u8);
    }
    out.extend_from_slice(bytes);
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Invalid> {
        if self.0.len() < n {
            return Err(Invalid);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Invalid> {
        self.take(N)?.try_into().map_err(|_| Invalid)
    }

    fn u8_len(&mut self) -> Result<&'a [u8], Invalid> {
        let [n] = self.array::<1>()?;
        self.take(n.into())
    }

    fn u16_len(&mut self) -> Result<&'a [u8], Invalid> {
        let n = u16::from_be_bytes(self.array()?);
        self.take(n.into())
    }

    fn u32_len(&mut self, max: usize) -> Result<&'a [u8], Invalid> {
        let n = u32::from_be_bytes(self.array()?) as usize;
        if n > max {
            return Err(Invalid);
        }
        self.take(n)
    }

    fn i64(&mut self) -> Result<i64, Invalid> {
        Ok(i64::from_be_bytes(self.array()?))
    }
}

impl Meta {
    /// The exact bytes authenticated with the text. Refuses values too
    /// long to encode.
    pub fn encode(&self) -> Result<Vec<u8>, Invalid> {
        if self.entry.len() > MAX_ENTRY_BYTES
            || self.content_id.len() > MAX_ID_BYTES
            || self.boot_id.len() > MAX_ID_BYTES
        {
            return Err(Invalid);
        }
        let mut out = Vec::new();
        put_field(&mut out, self.entry.as_bytes(), true);
        put_field(&mut out, &self.content_id, false);
        put_field(&mut out, self.boot_id.as_bytes(), false);
        for time in [self.created_wall_ms, self.created_monotonic_ms, self.deadline_wall_ms] {
            out.extend(time.to_be_bytes());
        }
        Ok(out)
    }

    /// Reads [`Self::encode`]'s output followed by zero padding.
    fn decode(bytes: &[u8]) -> Result<Self, Invalid> {
        let mut r = Reader(bytes);
        let entry = String::from_utf8(r.u16_len()?.to_vec()).map_err(|_| Invalid)?;
        let content_id = r.u8_len()?.to_vec();
        let boot_id = String::from_utf8(r.u8_len()?.to_vec()).map_err(|_| Invalid)?;
        let meta = Self {
            entry,
            content_id,
            boot_id,
            created_wall_ms: r.i64()?,
            created_monotonic_ms: r.i64()?,
            deadline_wall_ms: r.i64()?,
        };
        if r.0.iter().any(|&b| b != 0) {
            return Err(Invalid);
        }
        Ok(meta)
    }

    pub fn ttl_ms(&self) -> i64 {
        self.deadline_wall_ms.saturating_sub(self.created_wall_ms)
    }
}

fn pad(buf: &mut Vec<u8>, bucket: usize) {
    let padded = buf.len().div_ceil(bucket).max(1) * bucket;
    buf.resize(padded, 0);
}

fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    OsRng.fill_bytes(&mut out);
    out
}

fn ad(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    for part in parts {
        out.extend_from_slice(part);
    }
    out
}

/// One copy file, read and with its metadata authenticated. The text is
/// still sealed.
pub struct CopyFile {
    pub meta: Meta,
    pub wrapped_key: Vec<u8>,
    body_nonce: [u8; NONCE_LEN],
    body: Vec<u8>,
}

impl CopyFile {
    /// Seals `text` for `meta`. `wrap` seals the fresh data key with the
    /// device keystore; whatever it fails with is passed on.
    pub fn seal<E: From<Invalid>>(
        meta: Meta,
        text: &[u8],
        wrap: impl FnOnce(&[u8]) -> Result<Vec<u8>, E>,
    ) -> Result<Self, E> {
        let encoded = meta.encode()?;
        if text.len() > MAX_TEXT_BYTES {
            return Err(Invalid.into());
        }
        let data_key = Zeroizing::new(random::<KEY_LEN>());
        let wrapped_key = wrap(&*data_key)?;
        if wrapped_key.len() > MAX_WRAPPED_BYTES {
            return Err(Invalid.into());
        }

        // Sized up front, so neither padding nor the tag reallocates and
        // leaves an unwiped copy of the text behind.
        let padded = (4 + text.len()).div_ceil(BODY_BUCKET) * BODY_BUCKET;
        let mut body = Zeroizing::new(Vec::with_capacity(padded + TAG_LEN));
        body.extend((text.len() as u32).to_be_bytes());
        body.extend_from_slice(text);
        pad(&mut body, BODY_BUCKET);
        let body_nonce = random::<NONCE_LEN>();
        XChaCha20Poly1305::new((&*data_key).into())
            .encrypt_in_place(XNonce::from_slice(&body_nonce), &ad(&[&encoded]), &mut *body)
            .map_err(|_| Invalid)?;
        // Ciphertext from here on, so it needs no wiping.
        let body = std::mem::take(&mut *body);
        Ok(Self { meta, wrapped_key, body_nonce, body })
    }

    /// The hash the metadata's associated data covers.
    fn sealed_parts_hash(wrapped_key: &[u8], body_nonce: &[u8], body: &[u8]) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update((wrapped_key.len() as u32).to_be_bytes());
        hash.update(wrapped_key);
        hash.update(body_nonce);
        hash.update(body);
        hash.finalize().into()
    }

    /// The file's bytes, with the metadata sealed afresh under `keys`.
    pub fn to_bytes(&self, keys: &IndexKeys) -> Result<Vec<u8>, Invalid> {
        let mut meta = self.meta.encode()?;
        pad(&mut meta, META_BUCKET);
        let meta_nonce = random::<NONCE_LEN>();
        let parts = Self::sealed_parts_hash(&self.wrapped_key, &self.body_nonce, &self.body);
        keys.meta_cipher()
            .encrypt_in_place(XNonce::from_slice(&meta_nonce), &ad(&[&parts]), &mut meta)
            .map_err(|_| Invalid)?;

        let mut out = MAGIC.to_vec();
        out.extend(meta_nonce);
        out.extend((meta.len() as u32).to_be_bytes());
        out.extend(meta);
        out.extend((self.wrapped_key.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.wrapped_key);
        out.extend(self.body_nonce);
        out.extend_from_slice(&self.body);
        Ok(out)
    }

    /// Reads a copy file and authenticates its metadata with `keys`. Any
    /// difference from what paw wrote is [`Invalid`]. Never prompts.
    pub fn parse(keys: &IndexKeys, bytes: &[u8]) -> Result<Self, Invalid> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(Invalid);
        }
        let mut r = Reader(bytes);
        if r.take(MAGIC.len())? != MAGIC {
            return Err(Invalid);
        }
        let meta_nonce: [u8; NONCE_LEN] = r.array()?;
        let mut meta = r.u32_len(MAX_ENTRY_BYTES + 4 * META_BUCKET)?.to_vec();
        let wrapped_key = r.u32_len(MAX_WRAPPED_BYTES)?.to_vec();
        let body_nonce: [u8; NONCE_LEN] = r.array()?;
        let body = r.0.to_vec();
        if body.len() < TAG_LEN + BODY_BUCKET {
            return Err(Invalid);
        }

        let parts = Self::sealed_parts_hash(&wrapped_key, &body_nonce, &body);
        keys.meta_cipher()
            .decrypt_in_place(XNonce::from_slice(&meta_nonce), &ad(&[&parts]), &mut meta)
            .map_err(|_| Invalid)?;
        let meta = Meta::decode(&meta)?;
        Ok(Self { meta, wrapped_key, body_nonce, body })
    }

    /// Opens the text with the data key the keystore unwrapped, checking it
    /// was sealed for exactly this metadata.
    pub fn open_text(&self, data_key: &[u8]) -> Result<Zeroizing<Vec<u8>>, Invalid> {
        if data_key.len() != KEY_LEN {
            return Err(Invalid);
        }
        let encoded = self.meta.encode()?;
        let cipher = XChaCha20Poly1305::new(Key::from_slice(data_key));
        let mut body = Zeroizing::new(self.body.clone());
        cipher
            .decrypt_in_place(XNonce::from_slice(&self.body_nonce), &ad(&[&encoded]), &mut *body)
            .map_err(|_| Invalid)?;
        let mut r = Reader(&body);
        let len = u32::from_be_bytes(r.array()?) as usize;
        let text = r.take(len)?;
        if r.0.iter().any(|&b| b != 0) {
            return Err(Invalid);
        }
        let mut out = Zeroizing::new(Vec::with_capacity(len));
        out.extend_from_slice(text);
        Ok(out)
    }
}
