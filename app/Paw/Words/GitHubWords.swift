import Foundation

/// The words for a GitHub step that failed: one short line and the next
/// step.
enum GitHubWords {
    static func problem(_ error: Error) -> String {
        guard let error = error as? GitHubError else { return "Something went wrong. \(describe(error))" }
        switch error {
        case .Network:
            return "Couldn't reach GitHub. Check your connection and try again."
        case .DeviceFlowDisabled:
            return "GitHub sign-in is turned off for paw. Type the address instead."
        case .NotSignedIn, .SignedOut:
            return "Your GitHub sign-in ended. Sign in again."
        case .Vault:
            return Device.words("paw couldn't keep the sign-in on this iPhone. Try again.")
        case .Refused(let message), .Failed(let message):
            return "GitHub didn't allow it: \(message). Try again."
        case .StoreBusy:
            return "GitHub didn't allow it: \(storeBusyText). Try again."
        case .NotLinked:
            return "No store chosen yet. Choose one."
        case .WrongRepository(let store, _):
            return "This sign-in isn't for \(store)."
        }
    }

    /// Whether the person must sign in again.
    static func endedSignIn(_ error: Error) -> Bool {
        switch error as? GitHubError {
        case .NotSignedIn?, .SignedOut?: true
        default: false
        }
    }
}

extension GitHubError: WordedError {
    var sentence: String {
        GitHubWords.problem(self)
    }
}
