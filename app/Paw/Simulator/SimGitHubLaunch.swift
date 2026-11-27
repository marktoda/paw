import Foundation
import SwiftUI

#if DEBUG && targetEnvironment(simulator)
/// The core's simulated GitHub, for the GitHub screens and their UI tests.
/// `-pawSimGitHub <mode>` turns it on; without it the simulator has no
/// GitHub sign-in at all. It serves the simulated git host's store as
/// `mira/password-store`, and keeps the sign-in in memory (`SimTokenVault`):
/// an unsigned simulator build has no keychain. Modes:
/// - `signedOut`: nothing kept; sign-in waits until the debug control
///   authorises the code;
/// - `notInstalled`: paw's app is on no repository until the debug control
///   installs it;
/// - `many`: 30 other repositories beside the store;
/// - `denied`, `expired`: the first code is declined, or runs out; the
///   next waits as in `signedOut`.
enum SimGitHubMode: String {
    case signedOut, notInstalled, many, denied, expired
}

final class SimGitHubLaunch: @unchecked Sendable {
    static let shared = SimGitHubLaunch()

    static let repository = "mira/password-store"

    let mode: SimGitHubMode?
    let github: SimGitHub?
    let vault = SimTokenVault()
    private let lock = NSLock()
    private var firstSignInEnded = false

    private init() {
        let mode = LaunchArgument.simGitHub.string.flatMap(SimGitHubMode.init(rawValue:))
        self.mode = mode
        guard let mode, let host = SimRemoteHandle.shared.setupHost,
              let github = try? SimGitHub.start(remote: host, repository: Self.repository) else {
            github = nil
            return
        }
        switch mode {
        case .denied: github.setDeviceOutcome(outcome: .denyAfter(pending: 0))
        case .expired: github.setDeviceOutcome(outcome: .expireAfter(pending: 0))
        default: github.setDeviceOutcome(outcome: .waitForPerson)
        }
        if mode == .notInstalled { github.setInstalled(installed: false) }
        if mode == .many { github.setExtraRepositories(count: 30) }
        self.github = github
    }

    /// The store's sign-in against the simulated GitHub, or `nil` without
    /// `-pawSimGitHub`.
    func makeCore(stateDir: String) -> GitHubAccount? {
        github.map { $0.openAccount(stateDir: stateDir, vault: vault.asVault()) }
    }

    /// A sign-in ended: from now on codes wait for the debug control.
    fileprivate func signInEnded() {
        let first: Bool = lock.withLock {
            defer { firstSignInEnded = true }
            return !firstSignInEnded
        }
        if first { github?.setDeviceOutcome(outcome: .waitForPerson) }
    }

    /// The person entered the code on GitHub and pressed Authorize.
    func authorise() {
        github?.authorisePending()
    }

    /// The person chose the store on GitHub's install page.
    func install() {
        github?.setInstalled(installed: true)
    }

    /// The person removed paw on GitHub: no token works any more.
    func revoke() {
        github?.revokeAuthorisation()
    }
}

extension View {
    /// The simulated GitHub declines or lets expire only the first code
    /// (`denied`, `expired`): once `picker`'s first sign-in ends, codes
    /// wait for the debug control.
    func simEndsFirstSignIn(_ picker: GitHubPicker) -> some View {
        onChange(of: SimGitHubLaunch.waitsForCode(picker.stage)) { _, waits in
            if !waits { SimGitHubLaunch.shared.signInEnded() }
        }
    }
}

extension SimGitHubLaunch {
    fileprivate static func waitsForCode(_ stage: GitHubPicker.Stage) -> Bool {
        if case .code = stage { return true }
        return false
    }
}
#endif
