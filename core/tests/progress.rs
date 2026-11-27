//! The progress of a clone or fetch: at most ten reports a second, the end
//! always reported, nothing repeated, and nothing after a stop.

use std::time::{Duration, Instant};

use paw_core::sync::{PROGRESS_INTERVAL, ProgressThrottle, TransferProgress};

fn update(received: u32, total: u32) -> TransferProgress {
    TransferProgress { received_objects: received, total_objects: total, received_bytes: u64::from(received) * 100 }
}

#[test]
fn a_thousand_updates_in_a_second_are_reported_at_most_eleven_times_with_the_end() {
    let start = Instant::now();
    let mut throttle = ProgressThrottle::default();
    let mut reported = Vec::new();
    for received in 1..=1000u32 {
        let at = start + Duration::from_millis(u64::from(received));
        if throttle.should_report(at, update(received, 1000)) {
            reported.push((at, received));
        }
    }
    // Then libgit2 goes on calling while it indexes, with the same counts.
    for later in 0..50 {
        assert!(!throttle.should_report(start + Duration::from_millis(1100 + later * 50), update(1000, 1000)));
    }

    assert!(reported.len() <= 11, "{} reports: {reported:?}", reported.len());
    assert_eq!(reported.first().map(|(_, r)| *r), Some(1));
    assert_eq!(reported.last().map(|(_, r)| *r), Some(1000));
    // Spaced by the interval, except the end.
    for pair in reported[..reported.len() - 1].windows(2) {
        assert!(pair[1].0 - pair[0].0 >= PROGRESS_INTERVAL, "{pair:?}");
    }
}

#[test]
fn a_report_is_not_repeated_and_nothing_follows_a_stop() {
    let start = Instant::now();
    let mut throttle = ProgressThrottle::default();
    assert!(throttle.should_report(start, update(0, 0)));
    // The same counts a while later: nothing new to say.
    assert!(!throttle.should_report(start + Duration::from_secs(1), update(0, 0)));
    assert!(throttle.should_report(start + Duration::from_secs(1), update(5, 10)));

    throttle.stop();

    assert!(!throttle.should_report(start + Duration::from_secs(5), update(6, 10)));
    assert!(!throttle.should_report(start + Duration::from_secs(6), update(10, 10)));
    assert!(throttle.stopped());
}

#[test]
fn bytes_alone_moving_are_reported_on_the_interval() {
    let start = Instant::now();
    let mut throttle = ProgressThrottle::default();
    let at = |ms| start + Duration::from_millis(ms);
    let bytes = |n| TransferProgress { received_objects: 3, total_objects: 10, received_bytes: n };
    assert!(throttle.should_report(at(0), bytes(1)));
    assert!(!throttle.should_report(at(50), bytes(2)));
    assert!(throttle.should_report(at(100), bytes(3)));
}
