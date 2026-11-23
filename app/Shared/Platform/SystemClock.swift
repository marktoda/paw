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
/// Read from the kernel on every call; nothing is stored.
///
/// 1. `kern.bootsessionuuid`: a UUID the kernel makes at each boot. The
///    core's seam notes say the iOS 18 sandbox refuses it; the simulator
///    and the check screen show whether this phone allows it.
/// 2. Otherwise `kern.boottime`, seconds only. The kernel moves the boot
///    time when the wall clock is set (and may when NTP steps it), so a
///    clock change then changes the id and ends every copy: it fails
///    closed, never stretching a copy.
/// 3. If neither can be read, a fresh random id per call, which ends every
///    copy each time.
enum BootSession {
    /// The id, and which source gave it (for the check screen).
    static func read() -> (id: String, source: String) {
        var why: [String] = []
        switch sysctlString("kern.bootsessionuuid") {
        case .success(let uuid) where !uuid.isEmpty:
            return ("session-" + uuid, "kern.bootsessionuuid")
        case .success:
            why.append("bootsessionuuid empty")
        case .failure(let errorNumber):
            why.append("bootsessionuuid errno \(errorNumber)")
        }
        var boot = timeval()
        var size = MemoryLayout<timeval>.size
        if sysctlbyname("kern.boottime", &boot, &size, nil, 0) == 0, boot.tv_sec > 0 {
            return ("boottime-\(boot.tv_sec)", "kern.boottime (\(why.joined(separator: ", ")))")
        }
        why.append("boottime errno \(errno)")
        return ("unknown-" + UUID().uuidString, "none (\(why.joined(separator: ", ")))")
    }

    /// A string sysctl, or the errno that refused it.
    private static func sysctlString(_ name: String) -> Result<String, Int32Error> {
        var size = 0
        guard sysctlbyname(name, nil, &size, nil, 0) == 0 else { return .failure(Int32Error(errno)) }
        guard size > 0, size <= 256 else { return .success("") }
        var buffer = [CChar](repeating: 0, count: size + 1)
        guard sysctlbyname(name, &buffer, &size, nil, 0) == 0 else { return .failure(Int32Error(errno)) }
        let text = String(decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
        return .success(text.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    struct Int32Error: Error, CustomStringConvertible {
        let value: Int32
        init(_ value: Int32) { self.value = value }
        var description: String { String(value) }
    }
}
