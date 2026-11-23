import Darwin
import Foundation

/// The phone's clocks: the device half of the core's `Clock` seam
/// (core/src/copies/seams.rs). Every call reads the system; nothing is
/// cached except constants (the tick rate).
final class SystemClock: Clock, @unchecked Sendable {
    /// Milliseconds since 1970 by the phone's clock, which the person can
    /// set. The core never trusts it alone.
    func wallMs() -> Int64 {
        let ms = (Date().timeIntervalSince1970 * 1000).rounded(.down)
        return Int64(exactly: ms) ?? 0
    }

    /// Milliseconds on `mach_continuous_time`, which Apple documents as
    /// "a clock that increments monotonically in tick units (starting at an
    /// arbitrary point), including while the system is asleep". It cannot
    /// be set and starts again at boot. `mach_absolute_time` and
    /// `ProcessInfo.systemUptime` stop while the phone sleeps, so a copy
    /// would outlive its length over a night; they are not used.
    /// (`clock_gettime(CLOCK_MONOTONIC_RAW)` reads the same clock on
    /// Darwin; the tick API is used because its documentation is explicit.)
    func monotonicMs() -> Int64 {
        let ticks = mach_continuous_time()
        let timebase = Self.timebase
        // ticks * numer / denom nanoseconds, without overflow.
        let product = ticks.multipliedFullWidth(by: UInt64(timebase.numer))
        let divisor = UInt64(timebase.denom) * 1_000_000
        guard product.high < divisor else { return Int64.max }
        let ms = divisor.dividingFullWidth(product).quotient
        return Int64(clamping: ms)
    }

    func bootId() -> String {
        BootSession.read().id
    }

    private static let timebase: mach_timebase_info_data_t = {
        var info = mach_timebase_info_data_t()
        guard mach_timebase_info(&info) == KERN_SUCCESS, info.numer > 0, info.denom > 0 else {
            // Never seen; nanosecond ticks are the documented fallback on
            // Intel and the safe guess.
            return mach_timebase_info_data_t(numer: 1, denom: 1)
        }
        return info
    }()
}

/// Which boot this is, the same in every process until the next restart.
///
/// `kern.bootsessionuuid` is not readable from the iOS 18 sandbox, so
/// this uses the restart check Apple DTS describes: a POSIX named
/// semaphore, which lives in the kernel and does not survive a reboot.
/// The first caller after a boot creates it (`O_CREAT | O_EXCL`) and writes
/// a new random id to a file in the App Group container; every later
/// caller finds the semaphore (`EEXIST`) and reads the file.
///
/// Failure modes, all ending copies rather than stretching them:
/// - The first caller cannot write the file: it removes the semaphore
///   again and returns a fresh random id, so the next call tries again.
/// - The file cannot be read (it is `Complete`-protected, so while the
///   phone is locked): a fresh random id, which ends every copy. Copies
///   are never read while locked.
/// - The semaphore cannot be used at all (the sandbox refuses): the boot
///   time from `kern.boottime`. The kernel moves that when the clock is
///   set, so a clock change also ends every copy.
enum BootSession {
    /// design.md 3.15's App Group; POSIX names are at most 31 bytes.
    static let appGroup = "group.com.marktoda.paw"
    private static let semaphoreNames = ["\(appGroup)/boot", "paw.boot"]

    /// The id, and where it came from (for the check screen).
    static func read() -> (id: String, source: String) {
        for name in semaphoreNames {
            if let created = sem_open(name, O_CREAT | O_EXCL, 0o600, 0) {
                sem_close(created)
                let id = "boot-" + UUID().uuidString
                if write(id, replacing: true) { return (id, "semaphore \(name), first since boot") }
                sem_unlink(name)
                return ("unsaved-" + UUID().uuidString, "semaphore \(name), could not save the id")
            }
            let error = errno
            guard error == EEXIST else { continue }
            // Another process may have created it a moment ago and not
            // written the file yet.
            for _ in 0..<20 {
                if let id = readFile() { return (id, "semaphore \(name)") }
                usleep(5_000)
            }
            // Created this boot but the file is gone (e.g. reinstalled since
            // boot): make one, without replacing one that just appeared.
            _ = write("boot-" + UUID().uuidString, replacing: false)
            if let id = readFile() { return (id, "semaphore \(name), id remade") }
            return ("unreadable-" + UUID().uuidString, "semaphore \(name), id file unreadable")
        }
        var boot = timeval()
        var size = MemoryLayout<timeval>.size
        if sysctlbyname("kern.boottime", &boot, &size, nil, 0) == 0, boot.tv_sec > 0 {
            return ("boottime-\(boot.tv_sec).\(boot.tv_usec)", "kern.boottime (no semaphore: errno \(errno))")
        }
        return ("unknown-" + UUID().uuidString, "none (errno \(errno))")
    }

    private static var fileURL: URL? {
        let base = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup)
            ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        return base?.appendingPathComponent("boot-id", isDirectory: false)
    }

    private static func readFile() -> String? {
        guard let url = fileURL, let data = try? Data(contentsOf: url),
              let text = String(data: data, encoding: .utf8), text.hasPrefix("boot-"),
              UUID(uuidString: String(text.dropFirst("boot-".count))) != nil else { return nil }
        return text
    }

    private static func write(_ id: String, replacing: Bool) -> Bool {
        guard let url = fileURL else { return false }
        do {
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            let options: Data.WritingOptions = replacing
                ? [.atomic, .completeFileProtection]
                : [.withoutOverwriting, .completeFileProtection]
            try Data(id.utf8).write(to: url, options: options)
            return true
        } catch {
            return false
        }
    }
}
