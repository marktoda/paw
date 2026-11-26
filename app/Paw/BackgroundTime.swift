import UIKit

/// Time from iOS to finish store work after paw leaves the screen. The
/// store lives in the App Group container, and iOS kills a process that is
/// suspended while it holds a file lock there (0xdead10cc). Everything that
/// can take the store lock in the app runs inside one of these, taken by
/// the one place each kind of write goes through: `StoreEdits.write`,
/// `ChangeQueue.change`, `GitHubAccess.run`, `SetupModel.run`, the key
/// jobs' `hold`, and the few single writes (taking a replaced history,
/// server keys, removing the store). When the time runs out first, the
/// expiration handler ends it, as iOS requires. App target only.
@MainActor
final class BackgroundTime {
    private var id = UIBackgroundTaskIdentifier.invalid

    private init(_ name: String) {
        id = UIApplication.shared.beginBackgroundTask(withName: name) { [weak self] in
            MainActor.assumeIsolated { self?.end() }
        }
    }

    /// Asks for time now; call `end()` when the work is done.
    static func begin(_ name: String) -> BackgroundTime {
        BackgroundTime(name)
    }

    /// Safe to call more than once.
    func end() {
        guard id != .invalid else { return }
        UIApplication.shared.endBackgroundTask(id)
        id = .invalid
    }

    static func run<T>(_ name: String, _ body: () throws -> T) rethrows -> T {
        let time = begin(name)
        defer { time.end() }
        return try body()
    }
}

/// A key task that touches the store (a send, a check for changes,
/// setup's download) keeps background time until it has really ended.
extension BackgroundTime: KeyTaskHold {}
