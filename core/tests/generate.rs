//! The password generator: what it draws from, that it draws evenly, that
//! required classes are there, and that the strength it reports is right.
//! Statistical tests use a seeded generator, so they always see the same
//! passwords.

use std::collections::HashMap;

use paw_core::generate::{
    LOOK_ALIKES, MAX_LENGTH, MIN_LENGTH, PasswordPolicy, PolicyError, SYMBOLS, alphabet, entropy_bits,
    generate, generate_with, uniform_below,
};
use rand::SeedableRng;
use rand::rngs::StdRng;

fn policy(length: u32) -> PasswordPolicy {
    PasswordPolicy { length, ..PasswordPolicy::default() }
}

fn classes_of(policy: &PasswordPolicy) -> Vec<Vec<char>> {
    let all = alphabet(policy).unwrap();
    let mut classes = vec![
        all.iter().copied().filter(char::is_ascii_lowercase).collect::<Vec<_>>(),
        all.iter().copied().filter(char::is_ascii_uppercase).collect(),
        all.iter().copied().filter(char::is_ascii_digit).collect(),
        all.iter().copied().filter(|c| SYMBOLS.contains(*c)).collect(),
    ];
    classes.retain(|c| !c.is_empty());
    classes
}

#[test]
fn the_default_is_twenty_characters_of_every_class_without_look_alikes() {
    let default = PasswordPolicy::default();
    assert_eq!(default.length, 20);
    assert!(default.symbols && default.exclude_look_alikes);
    assert_eq!(alphabet(&default).unwrap().len(), 66);

    let password = generate(&default).unwrap();
    assert_eq!(password.chars().count(), 20);
    for class in classes_of(&default) {
        assert!(password.chars().any(|c| class.contains(&c)), "a class is missing");
    }
}

#[test]
fn the_default_policys_entropy_is_reported_exactly() {
    // Counted here independently, in whole numbers: 66^20 fits in a u128.
    // Lower 24, upper 24, digits 8, symbols 10.
    let sizes = [24u128, 24, 8, 10];
    let total: u128 = sizes.iter().sum();
    let mut count: i128 = 0;
    for set in 0u32..16 {
        let left_out: u128 = (0..4).filter(|i| set & (1 << i) != 0).map(|i| sizes[i]).sum();
        let term = (total - left_out).pow(20) as i128;
        count += if set.count_ones() % 2 == 0 { term } else { -term };
    }
    let expected = (count as f64).log2();

    let reported = entropy_bits(&PasswordPolicy::default()).unwrap();
    assert!((reported - expected).abs() < 1e-9, "{reported} != {expected}");
    // About 120.9 bits for any 66-character string, less a fraction of a
    // bit for requiring every class.
    assert!((120.0..120.9).contains(&reported), "{reported}");
}

#[test]
fn lengths_outside_the_bounds_are_refused() {
    assert_eq!(generate(&policy(MIN_LENGTH - 1)).unwrap_err(), PolicyError::Length);
    assert_eq!(generate(&policy(MAX_LENGTH + 1)).unwrap_err(), PolicyError::Length);
    assert_eq!(generate(&policy(0)).unwrap_err(), PolicyError::Length);
    assert_eq!(generate(&policy(MIN_LENGTH)).unwrap().len(), MIN_LENGTH as usize);
    assert_eq!(generate(&policy(MAX_LENGTH)).unwrap().len(), MAX_LENGTH as usize);
}

#[test]
fn every_class_is_present_even_in_the_shortest_passwords() {
    let mut rng = StdRng::seed_from_u64(1);
    let shortest = policy(MIN_LENGTH);
    let classes = classes_of(&shortest);
    for _ in 0..2_000 {
        let password = generate_with(&mut rng, &shortest).unwrap();
        for class in &classes {
            assert!(password.chars().any(|c| class.contains(&c)));
        }
    }
}

#[test]
fn look_alikes_never_appear_when_excluded_and_do_when_allowed() {
    let mut rng = StdRng::seed_from_u64(2);
    let mut seen = String::new();
    for _ in 0..500 {
        let password = generate_with(&mut rng, &policy(40)).unwrap();
        assert!(!password.chars().any(|c| LOOK_ALIKES.contains(c)));
    }
    let allowed = PasswordPolicy { exclude_look_alikes: false, ..policy(40) };
    for _ in 0..500 {
        seen.push_str(&generate_with(&mut rng, &allowed).unwrap());
    }
    assert!(LOOK_ALIKES.chars().all(|c| seen.contains(c)));
}

#[test]
fn only_the_chosen_classes_are_used() {
    let mut rng = StdRng::seed_from_u64(3);
    let no_symbols = PasswordPolicy { symbols: false, ..policy(12) };
    for _ in 0..200 {
        let password = generate_with(&mut rng, &no_symbols).unwrap();
        assert!(password.chars().all(|c| c.is_ascii_alphanumeric() && !LOOK_ALIKES.contains(c)));
    }
}

/// The probability that a given position holds a given character of class
/// `k`, when every password with all classes present is equally likely: the
/// other `length - 1` positions must cover the remaining classes.
fn exact_marginal(sizes: &[f64], k: usize, length: i32) -> f64 {
    let total: f64 = sizes.iter().sum();
    let valid = |len: i32, required: &[usize]| -> f64 {
        let mut sum = 0.0;
        for set in 0u32..(1 << required.len()) {
            let left_out: f64 = required.iter().enumerate().filter(|(i, _)| set & (1 << i) != 0).map(|(_, c)| sizes[*c]).sum();
            let sign = if set.count_ones() % 2 == 0 { 1.0 } else { -1.0 };
            sum += sign * ((total - left_out) / total).powi(len);
        }
        sum
    };
    let all: Vec<usize> = (0..sizes.len()).collect();
    let others: Vec<usize> = all.iter().copied().filter(|c| *c != k).collect();
    (valid(length - 1, &others) / total) / valid(length, &all)
}

#[test]
fn characters_are_drawn_evenly_over_a_large_sample() {
    // Short passwords make the "every class" condition matter, so this also
    // checks that it skews nothing beyond what uniformity over valid
    // passwords implies.
    let checked = policy(8);
    let classes = classes_of(&checked);
    let sizes: Vec<f64> = classes.iter().map(|c| c.len() as f64).collect();
    let mut rng = StdRng::seed_from_u64(4);
    let samples = 40_000;
    let mut counts: HashMap<char, f64> = HashMap::new();
    for _ in 0..samples {
        for c in generate_with(&mut rng, &checked).unwrap().chars() {
            *counts.entry(c).or_default() += 1.0;
        }
    }

    let mut chi_square = 0.0;
    let mut cells = 0;
    for (k, class) in classes.iter().enumerate() {
        let expected = exact_marginal(&sizes, k, 8) * f64::from(samples) * 8.0;
        for c in class {
            let observed = counts.get(c).copied().unwrap_or(0.0);
            chi_square += (observed - expected).powi(2) / expected;
            cells += 1;
        }
    }
    // 65 degrees of freedom: the 99.99th percentile is about 117.
    assert_eq!(cells, 66);
    assert!(chi_square < 117.0, "chi-square {chi_square}");
    // And the marginals add up, as a check on the expectation itself.
    let sum: f64 = (0..classes.len()).map(|k| exact_marginal(&sizes, k, 8) * sizes[k]).sum();
    assert!((sum - 1.0).abs() < 1e-9);
}

#[test]
fn positions_are_drawn_evenly_too() {
    // Digits per position, over many passwords: no position is where "the
    // required digit" goes.
    let checked = policy(8);
    let mut rng = StdRng::seed_from_u64(5);
    let samples = 20_000;
    let mut per_position = [0.0f64; 8];
    for _ in 0..samples {
        for (i, c) in generate_with(&mut rng, &checked).unwrap().chars().enumerate() {
            if c.is_ascii_digit() {
                per_position[i] += 1.0;
            }
        }
    }
    let mean = per_position.iter().sum::<f64>() / 8.0;
    let chi_square: f64 = per_position.iter().map(|o| (o - mean).powi(2) / mean).sum();
    // 7 degrees of freedom: the 99.99th percentile is about 29.9.
    assert!(chi_square < 29.9, "chi-square {chi_square} for {per_position:?}");
}

#[test]
fn the_unbiased_choice_is_even_for_a_range_that_does_not_divide_two_to_the_32() {
    let mut rng = StdRng::seed_from_u64(6);
    let n = 66;
    let draws = 660_000;
    let mut counts = vec![0.0f64; n as usize];
    for _ in 0..draws {
        counts[uniform_below(&mut rng, n) as usize] += 1.0;
    }
    let expected = f64::from(draws) / f64::from(n);
    let chi_square: f64 = counts.iter().map(|o| (o - expected).powi(2) / expected).sum();
    assert!(chi_square < 117.0, "chi-square {chi_square}");
}

/// An RNG that returns values from the incomplete top block first: the
/// rejection must skip them rather than fold them onto small values.
struct Scripted(Vec<u32>);

impl rand::RngCore for Scripted {
    fn next_u32(&mut self) -> u32 {
        self.0.remove(0)
    }
    fn next_u64(&mut self) -> u64 {
        u64::from(self.next_u32())
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        dest.fill(0);
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
        dest.fill(0);
        Ok(())
    }
}

#[test]
fn values_from_the_incomplete_last_block_are_drawn_again() {
    // 2^32 = 65075262 * 66 + 4: the top four values are rejected.
    let mut rng = Scripted(vec![u32::MAX, u32::MAX - 3, u32::MAX - 4, 7]);
    assert_eq!(uniform_below(&mut rng, 66), (u32::MAX - 4) % 66);
    assert_eq!(rng.0, vec![7]);
}

#[test]
fn two_passwords_are_never_the_same() {
    let a = generate(&PasswordPolicy::default()).unwrap();
    let b = generate(&PasswordPolicy::default()).unwrap();
    assert_ne!(*a, *b);
}
