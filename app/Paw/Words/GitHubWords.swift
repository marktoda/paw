import Foundation

/// The words for a GitHub step that failed: one short line and the next
/// step.
enum GitHubWords {
    static func problem(_ error: Error) -> String {
        guard let error = error as? GitHubError else { return "Something went wrong. \(describe(error))" }
        switch error {
        case .Fault(.remote(let failure)):
            return remoteProblem(failure)
        case .DeviceFlowDisabled:
            return "GitHub sign-in is turned off for paw. Type the address instead."
        case .Vault:
            return Device.words("paw couldn't keep the sign-in on this iPhone. Try again.")
        case .NotListed(let fullName):
            return "GitHub didn't allow it: \(fullName) is not in the list; list the repositories again. Try again."
        case .Fault(.storeBusy):
            return StoreBusyWords.line
        case .Fault(let fault):
            return "GitHub didn't allow it: \(faultDetail(fault)). Try again."
        }
    }

    private static func remoteProblem(_ failure: RemoteFailure) -> String {
        switch failure {
        case .noConnection, .unreachable:
            return "Couldn't reach GitHub. Check your connection and try again."
        case .notSignedIn, .signedOut:
            return "Your GitHub sign-in ended. Sign in again."
        case .gitHubRefused(let message):
            return "GitHub didn't allow it: \(message). Try again."
        case .unauthorised:
            // Met here when listing repositories.
            return "GitHub didn't allow it: GitHub did not accept paw's sign-in. Try again."
        case .notLinked:
            return "No store chosen yet. Choose one."
        case .wrongRepository(let store, _):
            return "This sign-in isn't for \(store)."
        default:
            return "GitHub didn't allow it: \(remoteDetail(failure)). Try again."
        }
    }

    /// Whether the person must sign in again.
    static func endedSignIn(_ error: Error) -> Bool {
        guard case .Fault(.remote(let failure))? = error as? GitHubError else { return false }
        return signInEnded(failure)
    }
}

extension GitHubError: WordedError {
    var sentence: String {
        GitHubWords.problem(self)
    }
}
