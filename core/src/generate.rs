//! The Generate button: random passwords from a policy.
//!
//! Every character is drawn from the OS's cryptographic random number
//! generator ([`OsRng`]), with an unbiased choice from the alphabet (see
//! [`uniform_below`]). A policy can require each character class at least
//! once. That is done by rejection: a whole password is drawn uniformly
//! from the alphabet and drawn again if a required class is missing. The
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

/// The fewest characters a policy may ask for.
pub const MIN_LENGTH: u32 = 4;
/// The most characters a policy may ask for.
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

/// What to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct PasswordPolicy {
    /// Characters, from [`MIN_LENGTH`] to [`MAX_LENGTH`].
    pub length: u32,
    pub lowercase: bool,
    pub uppercase: bool,
    pub digits: bool,
    /// From [`SYMBOLS`].
    pub symbols: bool,
    /// Leave out `0 O 1 l I o`.
    pub exclude_look_alikes: bool,
    /// Every class that is on appears at least once.
    pub require_each_class: bool,
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
        Self {
            length: 20,
            lowercase: true,
            uppercase: true,
            digits: true,
            symbols: true,
            exclude_look_alikes: true,
            require_each_class: true,
        }
    }
}

/// Why a policy cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error("a password must be {MIN_LENGTH} to {MAX_LENGTH} characters long")]
    Length,
    #[error("choose at least one kind of character")]
    NoClasses,
}

/// The characters of each class that is on, look-alikes removed if asked.
fn classes(policy: &PasswordPolicy) -> Vec<Vec<char>> {
    [
        (policy.lowercase, LOWERCASE),
        (policy.uppercase, UPPERCASE),
        (policy.digits, DIGITS),
        (policy.symbols, SYMBOLS),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, chars)| {
        chars
            .chars()
            .filter(|c| !(policy.exclude_look_alikes && LOOK_ALIKES.contains(*c)))
            .collect()
    })
    .collect()
}

fn check(policy: &PasswordPolicy) -> Result<Vec<Vec<char>>, PolicyError> {
    if !(MIN_LENGTH..=MAX_LENGTH).contains(&policy.length) {
        return Err(PolicyError::Length);
    }
    let classes = classes(policy);
    if classes.is_empty() {
        return Err(PolicyError::NoClasses);
    }
    Ok(classes)
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

/// A password for `policy`, from the OS random number generator.
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
    // A class can only be required when the password has room for all.
    let required = policy.require_each_class && classes.len() <= length;
    loop {
        // Sized up front, so it never reallocates and leaves copies behind.
        let mut password = Zeroizing::new(String::with_capacity(length * 4));
        for _ in 0..length {
            password.push(alphabet[uniform_below(rng, n) as usize]);
        }
        let complete = !required
            || classes.iter().all(|class| password.chars().any(|c| class.contains(&c)));
        if complete {
            return Ok(password);
        }
    }
}

/// How many bits of entropy a password from `policy` has: log2 of the
/// number of passwords the policy allows, all equally likely.
///
/// With classes required, that number is counted by inclusion and
/// exclusion over the classes left out: the sum over every set S of
/// classes of (-1)^|S| * (alphabet minus S)^length.
pub fn entropy_bits(policy: &PasswordPolicy) -> Result<f64, PolicyError> {
    let classes = check(policy)?;
    let total: usize = classes.iter().map(Vec::len).sum();
    let length = f64::from(policy.length);
    let all = length * (total as f64).log2();
    if !policy.require_each_class || classes.len() as u32 > policy.length {
        return Ok(all);
    }
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
