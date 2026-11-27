import UIKit
import UniformTypeIdentifiers

/// What `Clipboard` uses of a pasteboard: the system's, or a test's.
@MainActor
protocol Pasteboard: AnyObject {
    var changeCount: Int { get }
    var items: [[String: Any]] { get set }
    func setItems(_ items: [[String: Any]], options: [UIPasteboard.OptionsKey: Any])
}

extension UIPasteboard: Pasteboard {}

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
    private let pasteboard: Pasteboard
    /// The system's own expiry, on real time (`Platform.pasteboardLifetime`).
    private let systemLifetime: TimeInterval

    init(clock: Clock, pasteboard: Pasteboard = UIPasteboard.general, systemLifetime: TimeInterval = lifetime) {
        self.clock = clock
        self.pasteboard = pasteboard
        self.systemLifetime = systemLifetime
    }

    func copy(_ value: String) {
        let expiry = Date().addingTimeInterval(systemLifetime)
        pasteboard.setItems(
            [[UTType.utf8PlainText.identifier: value]],
            options: [.localOnly: true, .expirationDate: expiry])
        ownChangeCount = pasteboard.changeCount
        expiresAtMs = clock.wallMs() + Int64(Self.lifetime * 1000)
    }

    /// Lock now: clears the pasteboard if paw filled it last.
    func clearIfOurs() {
        guard let ownChangeCount, pasteboard.changeCount == ownChangeCount else { return }
        pasteboard.items = []
        self.ownChangeCount = nil
        expiresAtMs = nil
    }

    /// On return to the foreground, and when time is seen to pass: the
    /// system expiry may not have fired while paw was suspended.
    func clearIfExpired() {
        guard let expiresAtMs, clock.wallMs() >= expiresAtMs else { return }
        clearIfOurs()
    }

    /// Text that is not a secret (a public key, a sign-in code, a report):
    /// not followed, so Lock now leaves it. `localOnly` keeps it off
    /// Universal Clipboard; `expiry` is the system's.
    static func copyNotSecret(_ text: String, localOnly: Bool, expiry: Date? = nil) {
        var options: [UIPasteboard.OptionsKey: Any] = [:]
        if localOnly { options[.localOnly] = true }
        if let expiry { options[.expirationDate] = expiry }
        UIPasteboard.general.setItems([[UTType.utf8PlainText.identifier: text]], options: options)
    }
}
