import Foundation
import Observation
import SwiftUI

/// The store's GitHub sign-in as the screens see it: whether one is kept,
/// which repository was chosen, and signing out. The core keeps the
/// tokens (in the vault it was given); nothing here ever holds one. Every
/// call into the core runs on `queue`, never on the main thread.
@MainActor @Observable
final class GitHubAccess {
    let core: GitHubAccount
    private(set) var signedIn = false
    private(set) var link: GitHubLink?
    let queue = DispatchQueue(label: "paw-github", qos: .userInitiated)

    init(core: GitHubAccount) {
        self.core = core
        reload()
    }

    /// Reads whether a sign-in is kept and which repository is chosen.
    /// Reads the vault and a file; sends nothing.
    func reload() {
        let core = core
        queue.async { [weak self] in
            // A vault that can't be read holds no sign-in paw can use:
            // checks go on with the key, and Settings offers to sign in.
            let signedIn = (try? core.isSignedIn()) ?? false
            let link = Handoff(value: core.linkedRepository())
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
        return isGithubRepository(address: address, fullName: link.fullName, sshAddress: link.sshAddress)
    }

    /// Forgets the sign-in on this iPhone and, with `revoke`, asks GitHub
    /// to revoke it too. `done` gets the core's report or why not.
    func signOut(revoke: Bool, done: @escaping @MainActor (Result<SignOutReport, Error>) -> Void) {
        let core = core
        queue.async { [weak self] in
            let result = Handoff(value: Result { try core.signOut(revoke: revoke) })
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.reload()
                    done(result.value)
                }
            }
        }
    }
}

/// Where the store's GitHub sign-in comes from.
enum GitHubSignInSource {
    /// The core's sign-in for the store in `stateDir`: the real GitHub on
    /// a device, the simulated one (if `-pawSimGitHub` asked for it) in the
    /// simulator. The one place it is opened, from any thread.
    static func account(stateDir: String) -> GitHubAccount? {
        #if DEBUG && targetEnvironment(simulator)
        return SimGitHubLaunch.shared.makeCore(stateDir: stateDir)
        #else
        return GitHubAccount.open(stateDir: stateDir, vault: KeychainTokenVault())
        #endif
    }

    /// The sign-in for the screens.
    @MainActor
    static func access(stateDir: URL) -> GitHubAccess? {
        account(stateDir: stateDir.path).map { GitHubAccess(core: $0) }
    }
}
