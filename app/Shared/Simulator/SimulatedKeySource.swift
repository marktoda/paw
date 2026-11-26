import Foundation

#if DEBUG && targetEnvironment(simulator)
/// A simulated key as the platform's `KeySource`: the launch's key
/// (`SimulatedKeyHolder`), which is in unless `-pawSimUnplugged` and goes
/// in by the debug control, or a key of a test's or the gallery's own,
/// always in.
final class SimulatedKeySource: KeySource, @unchecked Sendable {
    /// The launch's simulated key.
    static let launched = SimulatedKeySource(
        connect: { try SimulatedKeyHolder.shared.open() }, isIn: { SimulatedKeyHolder.shared.isPluggedIn })

    /// "Let the next open finish after Cancel": the next task that
    /// finishes while the sheet follows it ends as if Cancel had been
    /// tapped just as the card answered, past the point where the core can
    /// stop it. The sheet closes, and the task's end goes where a late end
    /// goes. The simulated key cannot be cancelled that late by itself.
    @MainActor var finishesNextAfterCancel = false

    private let open: @Sendable () throws -> CardConnection
    private let isIn: @Sendable () -> Bool

    init(connect: @escaping @Sendable () throws -> CardConnection, isIn: @escaping @Sendable () -> Bool = { true }) {
        open = connect
        self.isIn = isIn
    }

    var readerNames: [String] {
        ["Simulated key"]
    }

    func connect() throws -> CardConnection {
        try open()
    }

    @MainActor func follow(_ changed: @escaping @MainActor (Bool) -> Void) -> AnyObject? {
        changed(isIn())
        return PlugWatch(changed)
    }

    @MainActor func adapted<Step>(_ job: KeyJob<Step>) -> KeyJob<Step> {
        var job = job
        let ended = job.ended
        let letGo = job.letGo
        job.ended = { [weak self] end in
            guard let self, self.finishesNextAfterCancel, case .done = end else { return ended(end) }
            self.finishesNextAfterCancel = false
            letGo(end)
            return .close
        }
        return job
    }
}

/// Hears the launch's key go in (`SimulatedKeyHolder.plugIn`) until dropped.
private final class PlugWatch {
    private let token: NSObjectProtocol

    @MainActor init(_ changed: @escaping @MainActor (Bool) -> Void) {
        token = NotificationCenter.default.addObserver(
            forName: SimulatedKeyHolder.pluggedInNotification, object: nil, queue: .main
        ) { _ in
            MainActor.assumeIsolated { changed(true) }
        }
    }

    deinit {
        NotificationCenter.default.removeObserver(token)
    }
}
#endif
