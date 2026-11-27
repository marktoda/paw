import Foundation

#if DEBUG && targetEnvironment(simulator)
/// A simulated key as the platform's `KeyConnector`, and what the
/// simulator's controls do to it: touch it (`key`), plug it in, let it
/// answer when read. The controls reach it through the reader the
/// platform's key was handed to (`KeyReader.simulated`).
final class SimulatedKeyConnector: KeyConnector, @unchecked Sendable {
    /// The PIN of a simulated key unless `-pawSimPin` says another.
    static let defaultPin = "123456"

    /// The launch's key, one per launch so PIN tries carry across
    /// operations the way they do on a real key. `KeyLaunchArgument`
    /// configures it. The entry points hand it to `Platform.simulated`.
    static let launched: SimulatedKeyConnector = {
        let pin = KeyLaunchArgument.simPin.string ?? defaultPin
        let touch = KeyLaunchArgument.simTouch.string == nil || KeyLaunchArgument.simTouch.bool
        let timeout = KeyLaunchArgument.simTouchTimeoutMs.int ?? 15_000
        let key = SimulatedKey(pin: pin, touchRequired: touch, touchTimeoutMs: UInt32(clamping: max(timeout, 0)))
        if let tries = KeyLaunchArgument.simPinTries.int {
            key.setPinTriesLeft(tries: UInt8(clamping: tries))
        }
        if KeyLaunchArgument.simKdf.string == "unsupported" {
            // Any algorithm but 3 (iterated and salted SHA-256) is one paw can't follow.
            key.setPinKdf(algorithm: 7)
        }
        return SimulatedKeyConnector(
            key: key, pluggedIn: !KeyLaunchArgument.simUnplugged.bool,
            attachFailures: KeyLaunchArgument.simAttachFails.int ?? 0,
            connection: { SimulatedCardConnection(key: key) })
    }()

    /// Posted on the main queue, with the connector as its object, when
    /// its key goes in.
    fileprivate static let pluggedInNotification = Notification.Name("SimulatedKeyConnector.pluggedIn")

    let key: SimulatedKey

    private let connection: @Sendable () -> CardConnection
    private let lock = NSLock()
    private var attachFailuresLeft: Int
    private var plugged: Bool

    /// `key`, in unless `pluggedIn` is false; its first `attachFailures`
    /// reads fail. The core reads it through `connection`, by default the
    /// key's own.
    init(
        key: SimulatedKey, pluggedIn: Bool = true, attachFailures: Int = 0,
        connection: (@Sendable () -> CardConnection)? = nil
    ) {
        self.key = key
        if let connection {
            self.connection = connection
        } else {
            self.connection = { key.asConnection() }
        }
        attachFailuresLeft = max(attachFailures, 0)
        plugged = pluggedIn
        if !pluggedIn {
            key.unplug()
        }
    }

    var readerNames: [String] {
        ["Simulated key"]
    }

    /// A connection to the key, unless this read is one of the first
    /// `attachFailures`, which fail.
    func connect() throws -> CardConnection {
        if takeAttachFailure() {
            throw ConnectionError.Failed(message: "The key didn't answer while it was being read.")
        }
        return connection()
    }

    @MainActor func follow(_ changed: @escaping @MainActor (Bool) -> Void) -> AnyObject? {
        changed(isPluggedIn)
        return PlugWatch(self, changed)
    }

    var isPluggedIn: Bool {
        lock.withLock { plugged }
    }

    /// Puts the key in, as a person plugging it into the port does; every
    /// reader following it hears of it.
    func plugIn() {
        key.plugIn()
        lock.withLock { plugged = true }
        DispatchQueue.main.async {
            NotificationCenter.default.post(name: Self.pluggedInNotification, object: self)
        }
    }

    /// From now on the key answers when read: whatever kept it from
    /// answering has passed (it is still the same key, never replugged).
    func stopAttachFailures() {
        lock.withLock { attachFailuresLeft = 0 }
    }

    private func takeAttachFailure() -> Bool {
        lock.withLock {
            guard attachFailuresLeft > 0 else { return false }
            attachFailuresLeft -= 1
            return true
        }
    }
}

/// Hears `connector`'s key go in (`plugIn`) until dropped.
private final class PlugWatch {
    private let token: NSObjectProtocol

    @MainActor init(_ connector: SimulatedKeyConnector, _ changed: @escaping @MainActor (Bool) -> Void) {
        token = NotificationCenter.default.addObserver(
            forName: SimulatedKeyConnector.pluggedInNotification, object: connector, queue: .main
        ) { _ in
            MainActor.assumeIsolated { changed(true) }
        }
    }

    deinit {
        NotificationCenter.default.removeObserver(token)
    }
}

final class SimulatedCardConnection: CardConnection, @unchecked Sendable {
    private let key: CardConnection

    init(key: SimulatedKey) {
        self.key = key.asConnection()
    }

    func transmit(apdu: Data) throws -> Data {
        // A card exchange can take as long as the key's touch timeout: never
        // on the main thread. The UI tests run with this check on.
        assert(!Thread.isMainThread, "card traffic on the main thread")
        return try key.transmit(apdu: apdu)
    }
}

extension KeyReader {
    /// The simulated key this reader follows; `nil` for any other.
    var simulated: SimulatedKeyConnector? {
        source as? SimulatedKeyConnector
    }
}
#endif
