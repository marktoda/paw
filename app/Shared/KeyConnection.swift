import Foundation

/// Where the key comes from. On a device it is the YubiKey on the USB-C
/// port. In the simulator, which cannot see hardware, it is a simulated key
/// so every screen can still be reached and tested.
enum KeyConnection {
    /// Opens a connection. Blocks; call off the main thread.
    static func open(onTouchNeeded: @escaping @Sendable () -> Void) throws -> CardConnection {
        #if targetEnvironment(simulator)
        return SimulatedCardConnection(key: SimulatedKeyHolder.shared.key, onTouchNeeded: onTouchNeeded)
        #else
        return try UsbCardConnection(onTouchNeeded: onTouchNeeded)
        #endif
    }

    static var readerNames: [String] {
        #if targetEnvironment(simulator)
        return ["Simulated key"]
        #else
        return UsbCardConnection.slotNames()
        #endif
    }

    static var isSimulated: Bool {
        #if targetEnvironment(simulator)
        return true
        #else
        return false
        #endif
    }

    /// Stands in for touching the gold disc. Does nothing on a device.
    static func simulateTouch() {
        #if targetEnvironment(simulator)
        SimulatedKeyHolder.shared.key.touch()
        #endif
    }
}

#if targetEnvironment(simulator)
/// One simulated key per app launch, so PIN tries carry across operations
/// the way they do on a real key. Launch arguments configure it:
/// `-pawSimPin 123456 -pawSimTouch 1 -pawSimTouchTimeoutMs 15000`.
final class SimulatedKeyHolder: @unchecked Sendable {
    static let shared = SimulatedKeyHolder()
    let key: SimulatedKey

    private init() {
        let defaults = UserDefaults.standard
        let pin = defaults.string(forKey: "pawSimPin") ?? "123456"
        let touch = defaults.object(forKey: "pawSimTouch") == nil || defaults.bool(forKey: "pawSimTouch")
        let timeout = defaults.object(forKey: "pawSimTouchTimeoutMs") == nil
            ? 15_000 : defaults.integer(forKey: "pawSimTouchTimeoutMs")
        key = SimulatedKey(pin: pin, touchRequired: touch, touchTimeoutMs: UInt32(timeout))
    }
}

final class SimulatedCardConnection: CardConnection, @unchecked Sendable {
    private let key: SimulatedKey
    private let onTouchNeeded: @Sendable () -> Void

    init(key: SimulatedKey, onTouchNeeded: @escaping @Sendable () -> Void) {
        self.key = key
        self.onTouchNeeded = onTouchNeeded
    }

    func transmit(apdu: Data) throws -> Data {
        key.exchange(apdu: apdu)
    }

    func touchNeeded() {
        onTouchNeeded()
    }
}
#endif

/// Turns a failure into a sentence a person can act on.
func describe(_ error: Swift.Error) -> String {
    guard let error = error as? PawError else {
        return error.localizedDescription
    }
    switch error {
    case .WrongPin(let triesLeft):
        return triesLeft == 1
            ? "Wrong PIN. 1 try left before the key locks."
            : "Wrong PIN. \(triesLeft) tries left."
    case .PinBlocked:
        return "The PIN is locked. Unlock it with your admin PIN."
    case .Transport(let message):
        return "Could not talk to the key: \(message)"
    case .NotAnEntry(let message):
        return "This is not an encrypted password: \(message)"
    case .Decrypt(let message):
        return "The key did not decrypt this: \(message)"
    case .Card(let message):
        return "The key reported a problem: \(message)"
    }
}
