import Foundation
import Observation
import SwiftUI

/// The store's GitHub sign-in as the screens see it: whether one is kept,
/// which repository was chosen, and signing out. The core keeps the
/// tokens (in the vault it was given); nothing here ever holds one. Every
/// call into the core runs on `queue`, never on the main thread.
@MainActor @Observable
final class GitHubAccess {
    let core: GitHub
    private(set) var signedIn = false
    private(set) var link: GitHubLink?
    let queue = DispatchQueue(label: "paw-github", qos: .userInitiated)

    init(core: GitHub) {
        self.core = core
        reload()
    }

    /// Reads whether a sign-in is kept and which repository is chosen.
    /// Reads the vault and a file; sends nothing.
    func reload() {
        let core = core
        queue.async { [weak self] in
            let signedIn = (try? core.isSignedIn()) ?? false
            let link = GitHubHandoff(value: core.linkedRepository())
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.signedIn = signedIn
                    self?.link = link.value
                }
            }
        }
    }

    /// Whether "Check for changes" on the store at `address` can use the
    /// sign-in instead of the key. The core checks again before it sends
    /// anything.
    func covers(address: String) -> Bool {
        guard signedIn, let link else { return false }
        return Self.isRepository(address, fullName: link.fullName, sshAddress: link.sshAddress)
    }

    /// Whether `address` (as setup saved it) is the GitHub repository
    /// `fullName`, whose SSH address is `sshAddress`.
    nonisolated static func isRepository(_ address: String, fullName: String, sshAddress: String) -> Bool {
        let typed = address.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if typed == sshAddress.lowercased() { return true }
        var path = typed
        if path.hasSuffix(".git") { path.removeLast(4) }
        let name = fullName.lowercased()
        return ["github.com:\(name)", "github.com/\(name)", "ssh.github.com:443/\(name)"].contains { path.hasSuffix($0) }
    }

    /// Forgets the sign-in on this iPhone and, with `revoke`, asks GitHub
    /// to revoke it too. `done` gets the core's report or why not.
    func signOut(revoke: Bool, done: @escaping @MainActor (Result<SignOutReport, Error>) -> Void) {
        let core = core
        queue.async { [weak self] in
            let result = GitHubHandoff(value: Result { try core.signOut(revoke: revoke) })
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.reload()
                    done(result.value)
                }
            }
        }
    }
}

extension EnvironmentValues {
    @Entry var gitHubAccess: GitHubAccess? = nil
}

/// Wraps a value that crosses from a work queue to the main actor once.
struct GitHubHandoff<Value>: @unchecked Sendable {
    let value: Value
}

/// What a background task will call to bring in changes with the GitHub
/// sign-in, with nobody present. Not called yet: it needs a registered
/// background task and the store in the App Group container first.
enum GitHubRefresh {
    /// Blocks for at most `seconds`: call it off the main thread. `nil` if
    /// paw's folder can't be found (or, in the simulator, there is no
    /// simulated GitHub).
    static func runBackground(seconds: UInt32 = 25) -> BackgroundRefreshReport? {
        guard let location = try? StoreLocation.standard() else { return nil }
        #if targetEnvironment(simulator)
        guard let core = SimGitHubLaunch.shared.makeCore(stateDir: location.stateDir.path) else { return nil }
        #else
        let core = GitHub.open(stateDir: location.stateDir.path, vault: KeychainTokenVault())
        #endif
        return core.backgroundRefresh(seconds: seconds)
    }

    /// The sign-in for the store in `stateDir`: the real GitHub on a
    /// device, the simulated one (if `-pawSimGitHub` asked for it) in the
    /// simulator.
    @MainActor
    static func access(stateDir: URL) -> GitHubAccess? {
        #if targetEnvironment(simulator)
        return SimGitHubLaunch.shared.makeCore(stateDir: stateDir.path).map { GitHubAccess(core: $0) }
        #else
        return GitHubAccess(core: GitHub.open(stateDir: stateDir.path, vault: KeychainTokenVault()))
        #endif
    }
}

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
            return "paw couldn't keep the sign-in on this iPhone. Try again."
        case .Refused(let message), .Failed(let message):
            return "GitHub didn't allow it: \(message). Try again."
        case .NotLinked:
            return "No repository is chosen yet. Choose one."
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
