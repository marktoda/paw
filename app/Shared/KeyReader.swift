import Foundation
import Observation
import SwiftUI

enum KeyReadProblem {
    /// The device has no smart card support at all.
    case unsupported
    /// Opening the key's session failed.
    case failed(Error)
}

/// The key going in and out. Follows the platform's key source and reads a
/// key that is in into the port's session: no PIN, no touch. A key task
/// waiting at Plug in goes on by itself once the key is read; a read that
/// failed is kept for the sheet's Plug in stage and its Try again.
@MainActor @Observable
final class KeyReader {
    let port: KeyPort
    let source: any KeyConnector
    private(set) var present = false
    /// Why the last read failed; `nil` once one works.
    private(set) var failure: Error?
    /// The system has no smart card support: this device can't use USB keys.
    private(set) var unsupported = false
    /// After each read, with the session it found or opened, or why it
    /// failed.
    @ObservationIgnored var onRead: ((Result<KeySession, Error>) -> Void)?

    @ObservationIgnored private var attempt = 0
    @ObservationIgnored private var watching = false
    /// `stopWatching` ran: nothing queued before it reads the key or
    /// follows a reader again.
    @ObservationIgnored private var stopped = false
    @ObservationIgnored private var following: AnyObject?
    @ObservationIgnored private let queue = DispatchQueue(label: "paw-card", qos: .userInitiated)

    init(port: KeyPort, source: any KeyConnector) {
        self.port = port
        self.source = source
    }

    var problem: KeyReadProblem? {
        if unsupported { return .unsupported }
        return failure.map { .failed($0) }
    }

    /// `problem` in `table`'s words.
    func problemWords(table: WordTable) -> KeyErrorWords? {
        problem.map { KeyErrorWords($0, table: table) }
    }

    /// Starts following the key, once. A key that is in is read now, and
    /// whenever one comes in: a session already open is kept, with the
    /// PIN it holds, and heard of again (`onRead`).
    func watch() {
        guard !watching, !stopped else { return }
        watching = true
        following = source.follow { [weak self] present in
            self?.keyChanged(present: present)
        }
        if following == nil {
            unsupported = true
        }
    }

    /// Stops following the key, for good: the screen is going away.
    func stopWatching() {
        stopped = true
        following = nil
    }

    /// paw went to the background or came back. Away, the key's session
    /// counts its time from now, for the core's PIN timeout; back, it hears
    /// so and may drop a PIN held too long, and a key that is in but has no
    /// session is read: by `readKey` when given, else by `read()`.
    func scenePhaseChanged(_ phase: ScenePhase, readKey: (() -> Void)? = nil) {
        switch phase {
        case .background:
            port.session()?.noteBackgrounded()
        case .active:
            port.session()?.noteForegrounded()
            guard needsRead else { return }
            if let readKey {
                readKey()
            } else {
                read()
            }
        default:
            break
        }
    }

    func readIfNeeded() {
        if needsRead {
            read()
        }
    }

    private var needsRead: Bool {
        present && port.session() == nil
    }

    /// Reads the key. A session already open is kept, with the PIN it
    /// holds: every retry (Try again, the foreground, a task starting) can
    /// land here, and reading the key again would end the session a task
    /// is using. Only the latest read's answer counts.
    func read() {
        guard !stopped else { return }
        attempt += 1
        let attempt = attempt
        let port = port
        let source = source
        queue.async {
            let result: Result<KeySession, Error> = Result {
                try port.session() ?? port.pluggedIn(connection: source.connect())
            }
            let handoff = Handoff(value: result)
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    guard let self, attempt == self.attempt else { return }
                    self.readFinished(handoff.value)
                }
            }
        }
    }

    /// Try again after a failed read.
    func retry() {
        failure = nil
        read()
    }

    private func keyChanged(present: Bool) {
        guard !stopped else { return }
        self.present = present
        if present {
            read()
        } else {
            wentAway()
        }
    }

    private func readFinished(_ result: Result<KeySession, Error>) {
        switch result {
        case .success:
            failure = nil
        case .failure(let error):
            // A key that cannot be read leaves any task at Plug in, which
            // is still true: no key paw can use is plugged in.
            failure = error
        }
        onRead?(result)
    }

    /// The key went away: its session ends and the core drops the PIN. A
    /// task using it fails with "Your key was unplugged".
    private func wentAway() {
        attempt += 1
        failure = nil
        port.unplugged()
    }
}
