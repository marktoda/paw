import Foundation

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

/// Where words are shown, when that changes them: AutoFill says some
/// things more shortly than the app.
enum ShownIn {
    case app, autoFill
}

/// An error that says itself in one sentence, where a screen has room for
/// one line. The app's own error types conform where it words them.
protocol WordedError: Swift.Error {
    var sentence: String { get }
}

/// Turns a failure into a sentence a person can act on, in this device's
/// words.
func describe(_ error: Swift.Error) -> String {
    Device.words(oneLine(error))
}

private func oneLine(_ error: Swift.Error) -> String {
    if let error = error as? WordedError {
        return error.sentence
    }
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
    case .HostKeyBundled(let host):
        return hostKeyBundled(host)
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
    case .InvalidField(let message):
        return message
    case .CopyUnavailable:
        return "This password needs your key again."
    case .FaceIdCancelled, .FaceIdNotRecognised, .FaceIdLockedOut, .FaceIdNotSetUp, .FaceIdChanged:
        return (FaceIdWords.problem(error) ?? "") + "."
    case .CopyFailed(let message):
        return "Could not keep this password for Face ID: " + message
    }
}

/// Why a host's key can't be forgotten or replaced: paw ships with it.
func hostKeyBundled(_ host: String) -> String {
    "\(host)'s keys come with paw and can't be forgotten. A different key means this connection isn't reaching "
        + "the real \(host): try another network"
}

/// Sentences in a row, leaving out the empty ones.
func sentences(_ parts: String...) -> String {
    parts.filter { !$0.isEmpty }.joined(separator: " ")
}

/// "it has no decryption key" as a sentence: "It has no decryption key."
func asSentence(_ text: String) -> String {
    let capitalised = capitalised(text)
    guard !capitalised.isEmpty else { return capitalised }
    return capitalised.hasSuffix(".") ? capitalised : capitalised + "."
}

/// `text` with its first letter capitalised.
func capitalised(_ text: String) -> String {
    text.prefix(1).uppercased() + text.dropFirst()
}
