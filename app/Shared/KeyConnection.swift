import Foundation

/// Where the key comes from. On a device it is the YubiKey on the USB-C
/// port. In the simulator, which cannot see hardware, it is a simulated key
/// so every screen can still be reached and tested.
enum KeyConnection {
    /// A connection for the key session (`KeyPort.pluggedIn`). Blocks; call
    /// off the main thread.
    static func open() throws -> CardConnection {
        #if targetEnvironment(simulator)
        if SimulatedKeyHolder.shared.takeAttachFailure() {
            throw ConnectionError.Failed(message: "The key didn't answer while it was being read.")
        }
        return SimulatedCardConnection(key: SimulatedKeyHolder.shared.key)
        #else
        return try UsbCardConnection()
        #endif
    }

    /// The simulated key is out at launch (`-pawSimUnplugged`). Always
    /// `false` on a device, where the system says whether a key is in.
    static var startsUnplugged: Bool {
        #if targetEnvironment(simulator)
        return SimulatedKeyHolder.shared.startsUnplugged
        #else
        return false
        #endif
    }

    static var readerNames: [String] {
        #if targetEnvironment(simulator)
        return ["Simulated key"]
        #else
        return UsbCardConnection.slotNames()
        #endif
    }

    /// Stands in for touching the gold disc. Does nothing on a device.
    static func simulateTouch() {
        #if targetEnvironment(simulator)
        SimulatedKeyHolder.shared.key.touch()
        #endif
    }

    /// The simulated key's touch timeout runs out now. Does nothing on a device.
    static func simulateTouchTimeout() {
        #if targetEnvironment(simulator)
        SimulatedKeyHolder.shared.key.timeOutTouch()
        #endif
    }

    /// The simulated key, failing its reads (`-pawSimAttachFails`), answers
    /// from now on. Does nothing on a device.
    static func simulateKeyAnswersAgain() {
        #if DEBUG && targetEnvironment(simulator)
        SimulatedKeyHolder.shared.stopAttachFailures()
        #endif
    }
}

#if targetEnvironment(simulator)
/// One simulated key per app launch, so PIN tries carry across operations
/// the way they do on a real key. Launch arguments configure it:
/// `-pawSimPin 123456 -pawSimTouch 1 -pawSimTouchTimeoutMs 15000`, and
/// `-pawSimPinTries 1` for a key another computer has already used tries on,
/// `-pawSimUnplugged 1` for a key that is not plugged in at launch,
/// `-pawSimAttachFails 2` for a key whose first two reads fail (as a real
/// key's can while it is still powering up), `-pawSimKdf unsupported` for a key
/// whose PIN KDF paw cannot follow.
final class SimulatedKeyHolder: @unchecked Sendable {
    static let shared = SimulatedKeyHolder()
    let key: SimulatedKey
    let startsUnplugged: Bool
    private let lock = NSLock()
    private var attachFailuresLeft: Int

    private init() {
        let pin = LaunchArguments.string("pawSimPin") ?? "123456"
        let touch = LaunchArguments.string("pawSimTouch") == nil || LaunchArguments.bool("pawSimTouch")
        let timeout = LaunchArguments.int("pawSimTouchTimeoutMs") ?? 15_000
        key = SimulatedKey(pin: pin, touchRequired: touch, touchTimeoutMs: UInt32(clamping: max(timeout, 0)))
        if let tries = LaunchArguments.int("pawSimPinTries") {
            key.setPinTriesLeft(tries: UInt8(clamping: tries))
        }
        if LaunchArguments.string("pawSimKdf") == "unsupported" {
            // Any algorithm but 3 (iterated and salted SHA-256) is one paw can't follow.
            key.setPinKdf(algorithm: 7)
        }
        attachFailuresLeft = max(LaunchArguments.int("pawSimAttachFails") ?? 0, 0)
        startsUnplugged = LaunchArguments.bool("pawSimUnplugged")
        if startsUnplugged {
            key.unplug()
        }
    }

    /// From now on the key answers when read: whatever kept it from
    /// answering has passed (it is still the same key, never replugged).
    func stopAttachFailures() {
        lock.withLock { attachFailuresLeft = 0 }
    }

    /// Whether this attach should fail, as `-pawSimAttachFails n` asks for
    /// the first n.
    func takeAttachFailure() -> Bool {
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
