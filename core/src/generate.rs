//! The Generate button: random passwords from a policy.
//!
//! Every character is drawn from the OS's cryptographic random number
//! generator ([`OsRng`]), with an unbiased choice from the alphabet (see
//! [`uniform_below`]). Lower and upper case letters and digits are always
//! in it, symbols if the policy says, and every class appears at least
//! once. That is done by rejection: a whole password is drawn uniformly
//! from the alphabet and drawn again if a class is missing. The
//! result is uniform over every password that meets the policy, so the
//! requirement costs only the few bits [`entropy_bits`] reports, and no
//! position or class is favoured the way "put one digit somewhere, fill
//! the rest" would favour them.
//!
//! There is no word mode: it needs a word list with a clear licence
//! embedded in the app, and none is included yet.

use rand::rngs::OsRng;
use rand::{CryptoRng, RngCore};
use zeroize::Zeroizing;

pub const MIN_LENGTH: u32 = 4;
pub const MAX_LENGTH: u32 = 128;

const LOWERCASE: &str = "abcdefghijklmnopqrstuvwxyz";
const UPPERCASE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &str = "0123456789";
/// Symbols almost every site accepts and every keyboard has on its first
/// symbol page: no quotes, backslash, space, brackets or `<>`, which some
/// sites refuse and some forms mangle.
pub const SYMBOLS: &str = "!#%+-.=?@_";
/// Characters that are easy to misread for one another in many fonts.
pub const LOOK_ALIKES: &str = "0O1lIo";

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct PasswordPolicy {
    /// Characters, from [`MIN_LENGTH`] to [`MAX_LENGTH`].
    pub length: u32,
    /// From [`SYMBOLS`].
    pub symbols: bool,
    /// Leave out `0 O 1 l I o`.
    pub exclude_look_alikes: bool,
}

impl Default for PasswordPolicy {
    /// 20 characters from lower and upper case letters, digits and
    /// [`SYMBOLS`], at least one of each, without look-alikes.
    ///
    /// Why: 20 characters from this 66-character alphabet carry about 120
    /// bits (see [`entropy_bits`]), beyond any offline guessing, while
    /// still fitting the length limits of nearly every site (16 is the
    /// commonest low cap; those sites need the policy changed anyway). One
    /// of each class satisfies the composition rules sites still enforce
    /// and costs under half a bit. Look-alikes are left out because a
    /// password on a phone is sometimes typed by hand into a TV or another
    /// computer; that costs about 2.5 bits. The symbol set is conservative
    /// for the same reason, and because sites differ on which symbols they
    /// allow.
    fn default() -> Self {
        Self { length: 20, symbols: true, exclude_look_alikes: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error("a password must be {MIN_LENGTH} to {MAX_LENGTH} characters long")]
    Length,
}

fn classes(policy: &PasswordPolicy) -> Vec<Vec<char>> {
    let symbols = if policy.symbols { SYMBOLS } else { "" };
    [LOWERCASE, UPPERCASE, DIGITS, symbols]
        .into_iter()
        .filter(|chars| !chars.is_empty())
        .map(|chars| chars.chars().filter(|c| !(policy.exclude_look_alikes && LOOK_ALIKES.contains(*c))).collect())
        .collect()
}

/// The classes `policy` draws from, if its length leaves room for one of
/// each: there are at most four, and [`MIN_LENGTH`] is four.
fn check(policy: &PasswordPolicy) -> Result<Vec<Vec<char>>, PolicyError> {
    if !(MIN_LENGTH..=MAX_LENGTH).contains(&policy.length) {
        return Err(PolicyError::Length);
    }
    Ok(classes(policy))
}

/// The alphabet a policy draws from, in a fixed order.
pub fn alphabet(policy: &PasswordPolicy) -> Result<Vec<char>, PolicyError> {
    Ok(check(policy)?.concat())
}

/// A number in `0..n`, every value equally likely. A plain `x % n` would
/// favour small values whenever `n` does not divide 2^32, so values from
/// the incomplete last block are drawn again (at most `n - 1` of 2^32
/// values, so almost never).
pub fn uniform_below<R: RngCore + ?Sized>(rng: &mut R, n: u32) -> u32 {
    debug_assert!(n > 0);
    // The largest multiple of n that fits, as a count of accepted values.
    let zone = u32::MAX - (u32::MAX - n + 1) % n;
    loop {
        let x = rng.next_u32();
        if x <= zone {
            return x % n;
        }
    }
}

pub fn generate(policy: &PasswordPolicy) -> Result<Zeroizing<String>, PolicyError> {
    generate_with(&mut OsRng, policy)
}

/// [`generate`] with the random number generator given, for tests.
pub fn generate_with<R: RngCore + CryptoRng + ?Sized>(
    rng: &mut R,
    policy: &PasswordPolicy,
) -> Result<Zeroizing<String>, PolicyError> {
    let classes = check(policy)?;
    let alphabet: Vec<char> = classes.concat();
    let n = alphabet.len() as u32;
    let length = policy.length as usize;
    loop {
        // Sized up front, so it never reallocates and leaves copies behind.
        let mut password = Zeroizing::new(String::with_capacity(length * 4));
        for _ in 0..length {
            password.push(alphabet[uniform_below(rng, n) as usize]);
        }
        if classes.iter().all(|class| password.chars().any(|c| class.contains(&c))) {
            return Ok(password);
        }
    }
}

/// How many bits of entropy a password from `policy` has: log2 of the
/// number of passwords the policy allows, all equally likely.
///
/// That number is counted by inclusion and exclusion over the classes
/// left out: the sum over every set S of classes of
/// (-1)^|S| * (alphabet minus S)^length.
pub fn entropy_bits(policy: &PasswordPolicy) -> Result<f64, PolicyError> {
    let classes = check(policy)?;
    let total: usize = classes.iter().map(Vec::len).sum();
    let length = f64::from(policy.length);
    let all = length * (total as f64).log2();
    // Each term as a fraction of total^length, so nothing overflows.
    let mut fraction = 0.0f64;
    for set in 0u32..(1 << classes.len()) {
        let left_out: usize =
            classes.iter().enumerate().filter(|(i, _)| set & (1 << i) != 0).map(|(_, c)| c.len()).sum();
        let sign = if set.count_ones() % 2 == 0 { 1.0 } else { -1.0 };
        fraction += sign * ((total - left_out) as f64 / total as f64).powf(length);
    }
    Ok(all + fraction.log2())
}

/// The Generate button's fixed settings.
#[uniffi::export]
pub fn default_password_policy() -> PasswordPolicy {
    PasswordPolicy::default()
}

#[uniffi::export]
pub fn password_entropy_bits(policy: PasswordPolicy) -> Result<f64, crate::write::WriteError> {
    Ok(entropy_bits(&policy)?)
}
