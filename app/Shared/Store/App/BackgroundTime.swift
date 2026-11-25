import UIKit

/// Time from iOS to finish store work after paw leaves the screen. The
/// store lives in the App Group container, and iOS kills a process that is
/// suspended while it holds a file lock there (0xdead10cc). Everything that
/// can take the store lock in the app (save, delete, rename, undo, send,
/// check for changes, setup's download and check, removing the store) runs
/// inside one of these. When the time runs out first, the expiration
/// handler ends it, as iOS requires. App target only.
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

    /// Runs `body` now, inside background time.
    static func run<T>(_ name: String, _ body: () throws -> T) rethrows -> T {
        let time = begin(name)
        defer { time.end() }
        return try body()
    }

    /// Keeps background time while a core key task runs (a send, a check
    /// for changes, setup's download), ending it once the task has ended.
    static func hold(_ name: String, while task: KeyTask?) {
        guard let task else { return }
        let time = begin(name)
        Task { @MainActor in
            while !isFinal(task.state()) {
                try? await Task.sleep(nanoseconds: 500_000_000)
            }
            time.end()
        }
    }

    private static func isFinal(_ state: KeyTaskState) -> Bool {
        switch state {
        case .done, .failed, .syncFailed, .setupFailed, .cancelled: true
        default: false
        }
    }
}
