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

    /// A connection for the key session (`KeyPort.pluggedIn`), which reports
    /// touches itself. Blocks; call off the main thread.
    static func openForSession() throws -> CardConnection {
        #if targetEnvironment(simulator)
        if SimulatedKeyHolder.shared.takeAttachFailure() {
            throw ConnectionError.Failed(message: "The key didn't answer while it was being read.")
        }
        #endif
        return try open(onTouchNeeded: {})
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

    /// The simulated key's touch timeout runs out now. Does nothing on a device.
    static func simulateTouchTimeout() {
        #if targetEnvironment(simulator)
        SimulatedKeyHolder.shared.key.timeOutTouch()
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
    private let onTouchNeeded: @Sendable () -> Void

    init(key: SimulatedKey, onTouchNeeded: @escaping @Sendable () -> Void) {
        self.key = key
        self.onTouchNeeded = onTouchNeeded
    }

    func transmit(apdu: Data) throws -> Data {
        // A card exchange can take as long as the key's touch timeout: never
        // on the main thread. The UI tests run with this check on.
        assert(!Thread.isMainThread, "card traffic on the main thread")
        return key.exchange(apdu: apdu)
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

