import Foundation
import Observation
import SwiftUI

/// The store's GitHub sign-in as the screens see it: whether one is kept,
/// which repository was chosen, and signing out. The core keeps the
/// tokens (in the vault it was given); nothing here ever holds one. Every
/// call into the core goes through `run` or `waitForCode`, off the main
/// thread, and `signedIn` and `link` are read again after each.
@MainActor @Observable
final class GitHubAccess {
    /// For reading only (diagnostics): calls go through `run`.
    let core: GitHubAccount
    private(set) var signedIn = false
    private(set) var link: GitHubLink?
    private let queue = DispatchQueue(label: "paw-github", qos: .userInitiated)
    /// Whether this GitHub hosts the store at an address.
    private let hosts: (_ address: String) -> Bool

    init(core: GitHubAccount, hosts: @escaping (_ address: String) -> Bool) {
        self.core = core
        self.hosts = hosts
        run { _ in }
    }

    /// Whether Settings offers the sign-in for the store at `address`: one
    /// is kept, a repository is chosen, or this GitHub hosts the store.
    func offered(for address: String) -> Bool {
        signedIn || link != nil || hosts(address)
    }

    /// Runs `work` on the core, then reads again whether a sign-in is kept
    /// and which repository is chosen (`work` may have changed either)
    /// before `done` hears how it went. Choosing a repository and checking
    /// for changes take the store lock: every call runs inside background
    /// time (`BackgroundTime`).
    func run<Value>(_ work: @escaping @Sendable (GitHubAccount) throws -> Value,
                    done: @escaping @MainActor (Result<Value, Error>) -> Void = { _ in }) {
        let core = core
        let time = BackgroundTime.begin("GitHub")
        queue.async { [weak self] in
            let result = Handoff(value: Result { try work(core) })
            let kept = GitHubAccess.kept(core)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    time.end()
                    self?.heard(kept)
                    done(result.value)
                }
            }
        }
    }

    /// Waits for the person to enter `signIn`'s code, on a thread of its
    /// own: each `poll` waits GitHub's interval itself.
    func waitForCode(_ signIn: DeviceSignIn, done: @escaping @MainActor (Result<SignInState, Error>) -> Void) {
        let core = core
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            var outcome: Result<SignInState, Error>
            while true {
                outcome = Result { try signIn.poll() }
                if case .success(.waiting) = outcome { continue }
                break
            }
            let result = Handoff(value: outcome)
            let kept = GitHubAccess.kept(core)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.heard(kept)
                    done(result.value)
                }
            }
        }
    }

    /// Whether a sign-in is kept and which repository is chosen. Reads the
    /// vault and a file; sends nothing.
    private nonisolated static func kept(_ core: GitHubAccount) -> Handoff<(signedIn: Bool, link: GitHubLink?)> {
        // A vault that can't be read holds no sign-in paw can use:
        // checks go on with the key, and Settings offers to sign in.
        Handoff(value: (signedIn: (try? core.isSignedIn()) ?? false, link: core.linkedRepository()))
    }

    private func heard(_ kept: Handoff<(signedIn: Bool, link: GitHubLink?)>) {
        signedIn = kept.value.signedIn
        link = kept.value.link
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
        run({ try $0.signOut(revoke: revoke) }, done: done)
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

    /// The sign-in for the screens. The simulated GitHub serves the
    /// simulated git host's store, whatever its address.
    @MainActor
    static func access(stateDir: URL) -> GitHubAccess? {
        #if DEBUG && targetEnvironment(simulator)
        let hosts: (String) -> Bool = { _ in true }
        #else
        let hosts: (String) -> Bool = { isGithubAddress(address: $0) }
        #endif
        return account(stateDir: stateDir.path).map { GitHubAccess(core: $0, hosts: hosts) }
    }
}
