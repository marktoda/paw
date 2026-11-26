import Foundation

/// What paw says wherever the store is busy: another part of paw, or the
/// AutoFill extension, held it longer than paw waits. Nothing was changed,
/// and trying again works.
enum StoreBusyWords {
    static let problem = "The store is busy"
    static let nextStep = "Try again in a moment."
    /// Where there is room for one line.
    static let line = "\(problem). \(nextStep)"
}

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
    // Cannot reach Swift: only reading the key's SSH key returns it, and
    // that is taken as "no authentication key". Listed so the switch stays
    // exhaustive.
    case .NoSshKey(let message):
        return "This key cannot log in to a git host: \(message)"
    case .UnsafePath(let message):
        return "Not a safe name for an entry: \(message)"
    case .Git(let message):
        return "Git reported a problem: \(message)"
    case .Store(let message):
        return message
    case .StoreBusy:
        return StoreBusyWords.line
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

/// Sentences in a row, leaving out the empty ones.
func sentences(_ parts: String...) -> String {
    parts.filter { !$0.isEmpty }.joined(separator: " ")
}
