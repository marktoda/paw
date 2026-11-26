import Foundation
import Observation
#if !(DEBUG && targetEnvironment(simulator))
import CryptoTokenKit
#endif

/// Why the key could not be read.
enum KeyReadProblem {
    /// The device has no smart card support at all.
    case unsupported
    /// Opening the key's session failed.
    case failed(Error)
}

/// The key going in and out. Follows the system's smart card readers (in
/// the simulator, the simulated key) and reads a key that is in into the
/// port's session: no PIN, no touch. A key task waiting at Plug in goes on
/// by itself once the key is read; a read that failed is kept for the
/// sheet's Plug in stage and its Try again.
@MainActor @Observable
final class KeyReader {
    typealias Connect = @Sendable () throws -> CardConnection

    let port: KeyPort
    private let connect: Connect
    /// A reader is there: on a device the system lists one; in the
    /// simulator the simulated key is plugged in.
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
    #if DEBUG && targetEnvironment(simulator)
    @ObservationIgnored private var plugWatch: NSObjectProtocol?
    #else
    @ObservationIgnored private var slotWatch: NSKeyValueObservation?
    @ObservationIgnored private var slotStateWatch: NSKeyValueObservation?
    #endif
    @ObservationIgnored private let queue = DispatchQueue(label: "paw-card", qos: .userInitiated)

    init(port: KeyPort, connect: @escaping Connect) {
        self.port = port
        self.connect = connect
    }

    var problem: KeyReadProblem? {
        if unsupported { return .unsupported }
        return failure.map { .failed($0) }
    }

    /// Starts following the readers, once. A key already in is read
    /// (in the simulator, the owner reads it when it needs to).
    func watch() {
        guard !watching, !stopped else { return }
        watching = true
        #if DEBUG && targetEnvironment(simulator)
        present = SimulatedKeyHolder.shared.isPluggedIn || port.session() != nil
        plugWatch = NotificationCenter.default.addObserver(
            forName: SimulatedKeyHolder.pluggedInNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, !self.stopped else { return }
                self.present = true
                self.readIfNeeded()
            }
        }
        #else
        // Minimal, and unverified on hardware: a reader appearing plugs in,
        // the list emptying unplugs. A key pulled out mid-command is also
        // caught by the core as a reader failure.
        guard let manager = TKSmartCardSlotManager.default else {
            unsupported = true
            return
        }
        // Called on whatever thread the system changes the list on.
        slotWatch = manager.observe(\.slotNames, options: [.initial, .new]) { [weak self] manager, _ in
            let present = !manager.slotNames.isEmpty
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.readersChanged(present: present)
                }
            }
        }
        #endif
    }

    /// Stops following the readers, for good: the screen is going away.
    func stopWatching() {
        stopped = true
        #if DEBUG && targetEnvironment(simulator)
        if let plugWatch {
            NotificationCenter.default.removeObserver(plugWatch)
        }
        plugWatch = nil
        #else
        slotWatch = nil
        slotStateWatch = nil
        #endif
    }

    /// Reads the key if one is in and has no session yet.
    func readIfNeeded() {
        if present, port.session() == nil {
            read()
        }
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
        let connect = connect
        queue.async {
            let result: Result<KeySession, Error> = Result {
                try port.session() ?? port.pluggedIn(connection: connect())
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

    #if !(DEBUG && targetEnvironment(simulator))
    private func readersChanged(present: Bool) {
        guard !stopped else { return }
        self.present = present
        if present {
            watchSlotState()
            readIfNeeded()
        } else {
            slotStateWatch = nil
            wentAway()
        }
    }

    /// Follows the first reader's card state, so a key that finishes
    /// powering up after its reader appeared is read then, without a
    /// replug. KVO on `TKSmartCardSlot.state`; unverified on hardware.
    private func watchSlotState() {
        guard let manager = TKSmartCardSlotManager.default, let name = manager.slotNames.first else { return }
        manager.getSlot(withName: name) { [weak self] slot in
            guard let slot else { return }
            let watch = slot.observe(\.state, options: [.new]) { [weak self] slot, _ in
                guard slot.state == .validCard else { return }
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        guard let self, !self.stopped else { return }
                        self.readIfNeeded()
                    }
                }
            }
            let handoff = Handoff(value: watch)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self, !self.stopped else { return }
                    self.slotStateWatch = handoff.value
                }
            }
        }
    }
    #endif
}
