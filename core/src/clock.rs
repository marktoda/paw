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
