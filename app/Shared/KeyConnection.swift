import Foundation

/// Where the key comes from. On a device it is the YubiKey on the USB-C
/// port. In the simulator's debug builds, which cannot see hardware, it is
/// a simulated key so every screen can still be reached and tested.
enum KeyConnection {
    /// A connection for the key session (`KeyPort.pluggedIn`). Blocks; call
    /// off the main thread.
    static func open() throws -> CardConnection {
        #if DEBUG && targetEnvironment(simulator)
        return try SimulatedKeyHolder.shared.open()
        #else
        return try UsbCardConnection()
        #endif
    }

    /// The simulated key is out at launch (`-pawSimUnplugged`). Always
    /// `false` on a device, where the system says whether a key is in.
    static var startsUnplugged: Bool {
        #if DEBUG && targetEnvironment(simulator)
        return SimulatedKeyHolder.shared.startsUnplugged
        #else
        return false
        #endif
    }

    static var readerNames: [String] {
        #if DEBUG && targetEnvironment(simulator)
        return ["Simulated key"]
        #else
        return UsbCardConnection.slotNames()
        #endif
    }
}

/// The words for a store another part of paw (or the AutoFill extension)
/// was using for longer than paw waits. Nothing was changed.
let storeBusyText = "the password store is busy: another part of paw (or the AutoFill extension) is using it; try again"

/// Whether `error` says the store was busy: trying again will work, and the
/// store is not broken.
func isStoreBusy(_ error: Swift.Error) -> Bool {
    if case .StoreBusy? = error as? PawError { return true }
    if case .StoreBusy? = error as? KeyError { return true }
    if case .StoreBusy? = error as? SyncError { return true }
    if case .StoreBusy? = error as? SetupError { return true }
    if case .StoreBusy? = error as? WriteError { return true }
    if case .StoreBusy? = error as? GitHubError { return true }
    return false
}

/// Turns a failure into a sentence a person can act on.
func describe(_ error: Swift.Error) -> String {
    guard let error = error as? PawError else {
        return error.localizedDescription
    }
    switch error {
    case .Transport(let message):
        return "Could not talk to the key: \(message)"
    case .NotAnEntry(let message):
        return "This is not an encrypted password: \(message)"
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
    case .StoreBusy:
        return storeBusyText
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
        return Device.words("Face ID was cancelled.")
    case .FaceIdNotRecognised:
        return Device.words("Face ID did not recognise you.")
    case .FaceIdLockedOut:
        return Device.words("Face ID is locked. Unlock your iPhone with its passcode, then try again.")
    case .FaceIdNotSetUp:
        return Device.words("Face ID is not set up on this iPhone.")
    case .FaceIdChanged:
        return Device.words("Face ID changed on this iPhone, so opened passwords were cleared.")
    case .CopyFailed(let message):
        return Device.words("Could not keep this password for Face ID: ") + message
    }
}

