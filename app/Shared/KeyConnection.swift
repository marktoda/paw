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
    // The git errors cannot reach Swift yet: nothing exported from the core
    // returns them. They are listed so the switch stays exhaustive.
    case .NoSshKey(let message):
        return "This key cannot log in to a git host: \(message)"
    case .Sign(let message):
        return "The key did not sign the login: \(message)"
    case .Network(let message):
        return "Could not reach the git host: \(message)"
    case .HostKeyMismatch(let presented):
        return "The git host's key has changed. It presented \(presented)."
    case .KeyRejected:
        return "The git host did not accept this key."
    case .NotFastForward:
        return "The remote has changes that are not on this device yet."
    case .UnsafePath(let message):
        return "Not a safe name for an entry: \(message)"
    case .Git(let message):
        return "Git reported a problem: \(message)"
    case .Store(let message):
        return message
    case .NotText:
        return "This entry is not text, so paw cannot show it."
    case .NotForThisKey(let recipients):
        return recipients.isEmpty
            ? "This entry is encrypted for another key."
            : "This entry is encrypted for another key: \(recipients.joined(separator: ", "))."
    case .InvalidField(let message):
        return message
    case .CopyUnavailable:
        return "This password needs your key again."
    case .FaceIdCancelled:
        return "Face ID was cancelled."
    case .FaceIdNotRecognised:
        return "Face ID did not recognise you."
    case .FaceIdLockedOut:
        return "Face ID is locked. Unlock your iPhone with its passcode, then try again."
    case .FaceIdNotSetUp:
        return "Face ID is not set up on this iPhone."
    case .FaceIdChanged:
        return "Face ID changed on this iPhone, so opened passwords were cleared."
    case .CopyFailed(let message):
        return "Could not keep this password for Face ID: \(message)"
    }
}
