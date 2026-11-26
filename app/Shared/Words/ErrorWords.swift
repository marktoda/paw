import Foundation

/// The words for a store another part of paw (or the AutoFill extension)
/// was using for longer than paw waits. Nothing was changed.
let storeBusyText = "the password store is busy: another part of paw (or the AutoFill extension) is using it; try again"

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
