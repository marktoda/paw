import UIKit
import UniformTypeIdentifiers

/// Puts secrets on the pasteboard for this device only, with a system
/// expiry, and remembers whether what is there is still paw's. One per
/// open store, on its clock.
@MainActor
final class Clipboard {
    static let lifetime: TimeInterval = 45

    private var ownChangeCount: Int?
    /// On `clock`, milliseconds since 1970.
    private var expiresAtMs: Int64?
    /// The copies' clock, so the simulated clock can end a copy in tests.
    /// The system's own expiry always runs on real time.
    private let clock: Clock

    init(clock: Clock) {
        self.clock = clock
    }

    /// The system's own expiry, on real time. In the simulator the UI tests
    /// can lengthen it (`-pawSimPasteboardSecs`): a test reads the
    /// pasteboard more than 45 real seconds after copying, and checks paw's
    /// own expiry on the simulated clock instead.
    private static var systemLifetime: TimeInterval {
        #if DEBUG && targetEnvironment(simulator)
        if let seconds = LaunchArgument.simPasteboardSecs.int {
            return TimeInterval(seconds)
        }
        #endif
        return lifetime
    }

    func copy(_ value: String) {
        let expiry = Date().addingTimeInterval(Self.systemLifetime)
        UIPasteboard.general.setItems(
            [[UTType.utf8PlainText.identifier: value]],
            options: [.localOnly: true, .expirationDate: expiry])
        ownChangeCount = UIPasteboard.general.changeCount
        expiresAtMs = clock.wallMs() + Int64(Self.lifetime * 1000)
    }

    /// Lock now: clears the pasteboard if paw filled it last.
    func clearIfOurs() {
        guard let ownChangeCount, UIPasteboard.general.changeCount == ownChangeCount else { return }
        UIPasteboard.general.items = []
        self.ownChangeCount = nil
        expiresAtMs = nil
    }

    /// On return to the foreground, and when time is seen to pass: the
    /// system expiry may not have fired while paw was suspended.
    func clearIfExpired() {
        guard let expiresAtMs, clock.wallMs() >= expiresAtMs else { return }
        clearIfOurs()
    }
}
