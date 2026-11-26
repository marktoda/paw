//! The clocks the core reads, implemented in Swift.

/// The phone's clocks: a copy's lifetime and the PIN's time in the
/// background are measured with them. Read from the system each call;
/// never cached.
///
/// On iOS:
/// - `wall_ms`: `Date().timeIntervalSince1970 * 1000`. The person can set
///   it, so the core never trusts it alone.
/// - `monotonic_ms`: `clock_gettime_nsec_np(CLOCK_MONOTONIC) / 1_000_000`.
///   On Darwin `CLOCK_MONOTONIC` keeps counting while the phone sleeps
///   (like `mach_continuous_time`, unlike `mach_absolute_time` and
///   `ProcessInfo.systemUptime`), cannot be set, and starts again at boot.
///   Apple DTS recommends it for trusted elapsed time; checked in Apple's
///   forums and the XNU sources, not yet on a device.
/// - `boot_id`: `kern.bootsessionuuid` is not usable: Apple removed it from
///   the iOS 18 sandbox and says it was never API. Use the restart check
///   Apple DTS describes instead: `sem_open("<group id>/boot", O_CREAT |
///   O_EXCL)`. If that creates the semaphore, this is the first run since
///   boot: write a new random UUID to a `Complete`-protected file in the
///   container and return it. If it fails with `EEXIST`, return the UUID
///   in the file (retrying briefly if the other process has not written it
///   yet). POSIX semaphores do not survive a reboot. Not yet tried on a
///   device. If the file has been tampered with, the monotonic checks in
///   the core still bound every copy's life.
#[uniffi::export(with_foreign)]
pub trait Clock: Send + Sync {
    /// Milliseconds since 1970, as the phone's clock says.
    fn wall_ms(&self) -> i64;
    /// Milliseconds since boot, counting sleep; never goes backwards within
    /// one boot.
    fn monotonic_ms(&self) -> i64;
    /// Changes on every reboot, is the same in every process until then.
    fn boot_id(&self) -> String;
}
