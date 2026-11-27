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
    return fault(of: error) == .storeBusy
}

/// What went wrong underneath a sync, setup, save or GitHub step, if it was
/// not the step's own refusal.
func fault(of error: Swift.Error) -> Fault? {
    if case .Fault(let fault)? = error as? SyncError { return fault }
    if case .Fault(let fault)? = error as? SetupError { return fault }
    if case .Fault(let fault)? = error as? WriteError { return fault }
    if case .Fault(let fault)? = error as? GitHubError { return fault }
    return nil
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
    case .NotAnEntry(let message):
        return "This is not an encrypted password: \(message)"
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

/// Lines read out as one label or announcement, each a sentence: a
/// headline without a full stop gets one, so VoiceOver pauses after it.
/// Empty lines are left out.
func spokenSentences(_ lines: String?...) -> String {
    lines.compactMap { line in
        guard let line, let last = line.last else { return nil }
        return ".?!:".contains(last) ? line : line + "."
    }.joined(separator: " ")
}
