import Foundation

#if DEBUG && targetEnvironment(simulator)
/// One simulated key per app launch, so PIN tries carry across operations
/// the way they do on a real key. `KeyLaunchArgument` configures it.
final class SimulatedKeyHolder: @unchecked Sendable {
    static let shared = SimulatedKeyHolder()
    /// Posted on the main queue when the key goes in.
    static let pluggedInNotification = Notification.Name("SimulatedKeyHolder.pluggedIn")
    let key: SimulatedKey
    private let lock = NSLock()
    private var attachFailuresLeft: Int
    private var plugged: Bool

    private init() {
        let pin = KeyLaunchArgument.simPin.string ?? "123456"
        let touch = KeyLaunchArgument.simTouch.string == nil || KeyLaunchArgument.simTouch.bool
        let timeout = KeyLaunchArgument.simTouchTimeoutMs.int ?? 15_000
        key = SimulatedKey(pin: pin, touchRequired: touch, touchTimeoutMs: UInt32(clamping: max(timeout, 0)))
        if let tries = KeyLaunchArgument.simPinTries.int {
            key.setPinTriesLeft(tries: UInt8(clamping: tries))
        }
        if KeyLaunchArgument.simKdf.string == "unsupported" {
            // Any algorithm but 3 (iterated and salted SHA-256) is one paw can't follow.
            key.setPinKdf(algorithm: 7)
        }
        attachFailuresLeft = max(KeyLaunchArgument.simAttachFails.int ?? 0, 0)
        plugged = !KeyLaunchArgument.simUnplugged.bool
        if !plugged {
            key.unplug()
        }
    }

    var isPluggedIn: Bool {
        lock.withLock { plugged }
    }

    /// Puts the key in, as a person plugging it into the port does; every
    /// reader watching hears of it.
    func plugIn() {
        key.plugIn()
        lock.withLock { plugged = true }
        DispatchQueue.main.async {
            NotificationCenter.default.post(name: Self.pluggedInNotification, object: nil)
        }
    }

    /// A connection to the key, unless this read is one of the first
    /// `-pawSimAttachFails n`, which fail.
    func open() throws -> CardConnection {
        if takeAttachFailure() {
            throw ConnectionError.Failed(message: "The key didn't answer while it was being read.")
        }
        return SimulatedCardConnection(key: key)
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

final class SimulatedCardConnection: CardConnection, @unchecked Sendable {
    private let key: SimulatedKey

    init(key: SimulatedKey) {
        self.key = key
    }

    func transmit(apdu: Data) throws -> Data {
        // A card exchange can take as long as the key's touch timeout: never
        // on the main thread. The UI tests run with this check on.
        assert(!Thread.isMainThread, "card traffic on the main thread")
        return key.exchange(apdu: apdu)
    }
}
#endif
