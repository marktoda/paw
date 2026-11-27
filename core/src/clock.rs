//! The clocks the core reads, implemented in Swift.

/// The phone's clocks: a copy's lifetime and the PIN's time in the
/// background are measured with them. Read from the system each call;
/// never cached.
#[uniffi::export(with_foreign)]
pub trait Clock: Send + Sync {
    /// Milliseconds since 1970, as the phone's clock says. The person can
    /// set it, so the core never trusts it alone.
    fn wall_ms(&self) -> i64;
    /// Milliseconds since boot, counting sleep; never goes backwards within
    /// one boot.
    fn monotonic_ms(&self) -> i64;
    /// Changes on every reboot, is the same in every process until then.
    /// If it can be tampered with, the monotonic checks still bound every
    /// copy's life.
    fn boot_id(&self) -> String;
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}
