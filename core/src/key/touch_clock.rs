//! Whether the next use of a key slot will wait for a touch.
//!
//! A slot whose policy is "cached" (a YubiKey's CACHED) takes one touch for
//! every use within 15 seconds of the last one. Asking for a touch the key
//! will not wait for teaches people to ignore the prompt, so paw says "touch
//! needed" only when it is sure, "may be needed" near the edge of the
//! window, and nothing when the policy and the clock say none is needed.

use std::time::{Duration, Instant};

use super::KeyTouch;

/// How long a cached touch lasts on a YubiKey.
const CACHE: Duration = Duration::from_secs(15);
/// Within this, a cached touch surely still holds: the margin covers the
/// time between the last use and the next one reaching the key.
const SURELY_CACHED: Duration = Duration::from_secs(10);

/// What to tell the person before a key operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TouchExpectation {
    NotNeeded,
    Needed,
    /// paw cannot tell: show the prompt, worded as "if it blinks".
    MayBeNeeded,
}

/// One slot's policy and when it last did something that took a touch.
#[derive(Debug, Clone, Copy)]
pub struct TouchClock {
    policy: KeyTouch,
    last_use: Option<Instant>,
}

impl TouchClock {
    pub fn new(policy: KeyTouch) -> Self {
        Self { policy, last_use: None }
    }

    /// Records a use that succeeded at `at`.
    pub fn used_at(&mut self, at: Instant) {
        self.last_use = Some(at);
    }

    pub fn expectation_at(&self, now: Instant) -> TouchExpectation {
        match self.policy {
            KeyTouch::Off => TouchExpectation::NotNeeded,
            KeyTouch::EachUse => TouchExpectation::Needed,
            KeyTouch::Unknown => TouchExpectation::MayBeNeeded,
            KeyTouch::Cached => match self.last_use.map(|at| now.saturating_duration_since(at)) {
                Some(since) if since < SURELY_CACHED => TouchExpectation::NotNeeded,
                Some(since) if since < CACHE + (CACHE - SURELY_CACHED) => {
                    TouchExpectation::MayBeNeeded
                }
                _ => TouchExpectation::Needed,
            },
        }
    }

    pub fn expectation(&self) -> TouchExpectation {
        self.expectation_at(Instant::now())
    }
}
