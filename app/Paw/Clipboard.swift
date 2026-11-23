import UIKit
import UniformTypeIdentifiers

/// Puts secrets on the pasteboard for this device only, with a system
/// expiry, and remembers whether what is there is still paw's.
@MainActor
enum Clipboard {
    static let lifetime: TimeInterval = 45

    private static var ownChangeCount: Int?
    /// On paw's clock (`useClock`), milliseconds since 1970.
    private static var expiresAtMs: Int64?
    /// The copies' clock, so the simulated clock can end a copy in tests.
    /// The system's own expiry always runs on real time.
    private static var nowMs: () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) }

    /// Measures paw's own expiry on `clock` (the one the Face ID copies use).
    static func useClock(_ clock: Clock) {
        nowMs = { clock.wallMs() }
    }

    static func copy(_ value: String) {
        let expiry = Date().addingTimeInterval(lifetime)
        UIPasteboard.general.setItems(
            [[UTType.utf8PlainText.identifier: value]],
            options: [.localOnly: true, .expirationDate: expiry])
        ownChangeCount = UIPasteboard.general.changeCount
        expiresAtMs = nowMs() + Int64(lifetime * 1000)
    }

    /// Lock now: clears the pasteboard if paw filled it last.
    static func clearIfOurs() {
        guard let ownChangeCount, UIPasteboard.general.changeCount == ownChangeCount else { return }
        UIPasteboard.general.items = []
        self.ownChangeCount = nil
        expiresAtMs = nil
    }

    /// On return to the foreground, and when time is seen to pass: the
    /// system expiry may not have fired while paw was suspended.
    static func clearIfExpired() {
        guard let expiresAtMs, nowMs() >= expiresAtMs else { return }
        clearIfOurs()
    }
}
