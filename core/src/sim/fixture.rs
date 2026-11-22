//! A small, made-up password store for the simulator and for tests.
//!
//! Most of it is encrypted for the simulated key, so it opens in the
//! simulator. One folder, `archive`, is encrypted for a key nobody holds:
//! that is what a store looks like after its owner moved to a new key and
//! never re-encrypted the old entries, and paw has to say so before it asks
//! for a touch.
//!
//! The same call always writes the same bytes: the encryption draws its
//! randomness from a fixed seed per entry. That is fine only because these
//! keys and passwords protect nothing.

use std::path::Path;

use pgp::crypto::hash::HashAlgorithm;
use pgp::crypto::public_key::PublicKeyAlgorithm;
use pgp::crypto::sym::SymmetricKeyAlgorithm;
use pgp::packet::{PubKeyInner, PublicKey};
use pgp::types::{EcdhKdfType, EcdhPublicParams, KeyDetails, KeyVersion, PublicParams, Timestamp};
use rand::SeedableRng;
use rand::rngs::StdRng;

use super::{CREATED, SimulatedKey};
use crate::PawError;

/// The folder whose entries the simulated key cannot open.
pub const OTHER_KEY_FOLDER: &str = "archive";

/// `(path, contents)` for every entry encrypted for the simulated key.
const ENTRIES: &[(&str, &str)] = &[
    ("Home Wi-Fi", "plum-cedar-otter-41\nnetwork: Maple House\nsecurity: WPA3\n"),
    ("phone-pin", "4071\n"),
    (
        "web/github.com",
        "tidal-Quartz-88-ferns\nlogin: you@example.com\nurl: https://github.com/login\n\
         otpauth://totp/GitHub:you?secret=JBSWY3DPEHPK3PXP&issuer=GitHub\n\
         recovery codes are in the blue notebook\n",
    ),
    ("web/gitlab.com", "Copper!lantern7\nlogin: you@example.com\nurl: https://gitlab.com\n"),
    ("web/news.ycombinator.com", "slow-violet-kettle\nusername: you\n"),
    ("web/example.com", "correct horse battery staple\nlogin: you@example.com\nurl: example.com\n"),
    ("web/accounts.example.org", "Nine-lazy-Rivers-2\nemail: you@example.com\n"),
    (
        "web/Zürich Tourism",
        "alpen-Glow-515\nlogin: you@example.com\nwebsite: https://zuerich.example\n",
    ),
    ("email/fastmail", "amber-Falcon-ridge-60\nlogin: you@example.com\nurl: https://app.fastmail.com\n"),
    ("email/work", "Granite+meadow+19\r\nuser: you@work.example\r\nurl: https://mail.work.example\r\n"),
    ("email/old-webmail", "velvet-anchor-3\nlogin: you.old@example.net"),
    (
        "bank/first-example-bank",
        "Harbor_73_willow\nlogin: 00417788\nurl: https://bank.example\n\
         security answer: Biscuit\ncard pin: 2580\nBranch closes at 4 on Fridays.\n",
    ),
    ("bank/credit-card", "mossy-Tiger-442\nlogin: you@example.com\nurl: https://cards.example\n"),
    ("bank/broker", "Juniper.sky.908\nlogin: you@example.com\n"),
    ("home/router", "admin-Pebble-66\nuser: admin\nurl: http://192.168.1.1\n"),
    ("home/alarm", "771204\nNot the duress code. That one ends in 9.\n"),
    ("home/nas", "Orchid#basalt#12\nlogin: you\nurl: https://nas.home.example:5001\n"),
    ("home/Guest Wi-Fi", "sunny-pancake-day\nnetwork: Maple House Guest\n"),
    ("work/vpn", "Fjord-lemon-204\nlogin: you@work.example\n"),
    ("work/wiki.example.com", "paper-Crane-31\nlogin: you@work.example\nurl: https://wiki.example.com\n"),
    ("work/Jira", "Timber&salt&58\nlogin: you@work.example\n"),
    ("work/payroll", "ocean-Marble-777\nlogin: you@work.example\nurl: https://payroll.work.example\n"),
    ("servers/example.net/root", "Kestrel/ember/5150\nuser: root\n"),
    ("servers/example.net/deploy", "birch-Signal-09\nuser: deploy\n"),
    ("servers/backup-box", "Lichen~comet~84\nuser: you\nport: 2222\n"),
    ("travel/rail.example", "Saffron-tunnel-6\nlogin: you@example.com\nurl: https://rail.example\n"),
    ("travel/airline", "cobalt-Heron-340\nlogin: you@example.com\nfrequent flyer: EX1234567\n"),
];

/// Entries encrypted for the other key only.
const OTHER_KEY_ENTRIES: &[(&str, &str)] = &[
    ("archive/old-forum", "first-password-ever\nlogin: you\n"),
    ("archive/university", "Lecture-hall-2009\nlogin: s0912345\n"),
];

/// A Curve25519 encryption key the simulated card does not hold. Its
/// private half is as public as this file.
pub fn other_key() -> Result<PublicKey, PawError> {
    let secret = x25519_dalek::StaticSecret::from([8u8; 32]);
    let params = PublicParams::ECDH(EcdhPublicParams::Curve25519Legacy {
        p: x25519_dalek::PublicKey::from(&secret),
        hash: HashAlgorithm::Sha256,
        alg_sym: SymmetricKeyAlgorithm::AES128,
        ecdh_kdf_type: EcdhKdfType::Native,
    });
    PubKeyInner::new(
        KeyVersion::V4,
        PublicKeyAlgorithm::ECDH,
        Timestamp::from_secs(CREATED),
        None,
        params,
    )
    .and_then(PublicKey::from_inner)
    .map_err(|e| PawError::Card(e.to_string()))
}

fn hex_fingerprint(key: &PublicKey) -> String {
    key.fingerprint().as_bytes().iter().map(|b| format!("{b:02X}")).collect()
}

/// Hex fingerprint of the key the `archive` folder is encrypted for.
pub fn other_key_fingerprint_hex() -> Result<String, PawError> {
    other_key().map(|key| hex_fingerprint(&key))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), PawError> {
    let failed = |e: std::io::Error| PawError::Store(format!("could not write the fixture store: {e}"));
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(failed)?;
    }
    std::fs::write(path, bytes).map_err(failed)
}

/// Writes the fixture store into `dir`, creating it if need be. Files
/// already there are overwritten, never removed.
#[uniffi::export]
pub fn write_fixture_store(dir: String) -> Result<(), PawError> {
    let root = Path::new(&dir);
    // Only read for its public key, so the PIN and policies do not matter.
    let card = SimulatedKey::new(String::new(), false, 0);
    let card_key = crate::card_decryption_key(card)?;
    let other_key = other_key()?;

    write(&root.join(".gpg-id"), format!("{}\n", hex_fingerprint(&card_key)).as_bytes())?;
    write(
        &root.join(OTHER_KEY_FOLDER).join(".gpg-id"),
        format!("# The key from the old laptop.\n{}\n", hex_fingerprint(&other_key)).as_bytes(),
    )?;

    let groups = [(ENTRIES, card_key), (OTHER_KEY_ENTRIES, other_key)];
    let mut seed = 0u64;
    for (entries, key) in groups {
        for (path, contents) in entries {
            seed += 1;
            let ciphertext = crate::encrypt_to_keys(
                StdRng::seed_from_u64(seed),
                std::slice::from_ref(&key),
                contents.as_bytes(),
            )?;
            write(&root.join(format!("{path}.gpg")), &ciphertext)?;
        }
    }
    Ok(())
}
