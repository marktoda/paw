import Foundation
import Observation

/// Signing in to GitHub with a one-time code, then choosing the store from
/// the repositories the person let paw see. The core does every step on a
/// work queue; the code is polled for on its own thread, as the core asks.
/// No token ever reaches this model.
@MainActor @Observable
final class GitHubPicker: Identifiable {
    enum Purpose {
        /// Setup: the chosen repository's address goes to setup.
        case pick(chosen: @MainActor (String) -> Void)
        /// Settings: sign in for the store at `address`, choosing its
        /// repository without asking.
        case link(address: String, linked: @MainActor () -> Void)
    }

    /// Why a sign-in ended without signing in.
    enum Ending: Equatable {
        case denied, expired, cancelled
        case failed(String)

        var words: String {
            switch self {
            case .denied: "You declined on GitHub."
            case .expired: "The code expired before you entered it."
            case .cancelled: "Sign-in cancelled."
            case .failed(let words): words
            }
        }
    }

    enum Stage: Equatable {
        case starting
        case code(DeviceCode)
        case ended(Ending)
        case listing
        case repositories([GitHubRepository], installUrl: String)
        /// No repository can be seen yet (`storeMissing`: none of them is
        /// the store's).
        case notInstalled(installUrl: String, storeMissing: Bool)
        case listFailed(String, signInAgain: Bool)
        case choosing(String)
    }

    private(set) var stage: Stage = .starting
    let access: GitHubAccess
    private let purpose: Purpose
    @ObservationIgnored private var signIn: DeviceSignIn?
    /// Bumped by every new step, so an answer to an older one is dropped.
    @ObservationIgnored private var generation = 0

    nonisolated var id: ObjectIdentifier { ObjectIdentifier(self) }

    init(access: GitHubAccess, purpose: Purpose) {
        self.access = access
        self.purpose = purpose
    }

    func begin() {
        if access.signedIn {
            list()
        } else {
            startSignIn()
        }
    }

    // MARK: Signing in

    /// Asks GitHub for a code (one request), then waits for the person.
    func startSignIn() {
        endSignIn()
        generation += 1
        let generation = generation
        stage = .starting
        access.run({ try $0.startSignIn() }) { [weak self] result in
            self?.started(result, generation: generation)
        }
    }

    private func started(_ result: Result<DeviceSignIn, Error>, generation: Int) {
        guard generation == self.generation else {
            if case .success(let late) = result { late.cancel() }
            return
        }
        switch result {
        case .success(let signIn):
            self.signIn = signIn
            stage = .code(signIn.code())
            poll(signIn, generation: generation)
        case .failure(let error):
            stage = .ended(.failed(GitHubWords.problem(error)))
        }
    }

    private func poll(_ signIn: DeviceSignIn, generation: Int) {
        access.waitForCode(signIn) { [weak self] result in
            self?.polled(result, generation: generation)
        }
    }

    private func polled(_ result: Result<SignInState, Error>, generation: Int) {
        guard generation == self.generation else { return }
        signIn = nil
        switch result {
        case .success(.signedIn):
            list()
        case .success(.denied):
            stage = .ended(.denied)
        case .success(.expired):
            stage = .ended(.expired)
        case .success(.cancelled), .success(.waiting):
            stage = .ended(.cancelled)
        case .failure(let error):
            stage = .ended(.failed(GitHubWords.problem(error)))
        }
    }

    func cancelSignIn() {
        guard case .code = stage else { return }
        endSignIn()
        generation += 1
        stage = .ended(.cancelled)
    }

    /// The sheet went away: a sign-in still waiting ends.
    func dismissed() {
        endSignIn()
        generation += 1
    }

    private func endSignIn() {
        signIn?.cancel()
        signIn = nil
    }

    // MARK: Choosing the repository

    func list() {
        generation += 1
        let generation = generation
        stage = .listing
        access.run({ try $0.repositories() }) { [weak self] result in
            self?.listed(result, generation: generation)
        }
    }

    private func listed(_ result: Result<RepositoryList, Error>, generation: Int) {
        guard generation == self.generation else { return }
        switch result {
        case .success(.notInstalled(let installUrl)):
            stage = .notInstalled(installUrl: installUrl, storeMissing: false)
        case .success(.repositories(let repositories, let installUrl)):
            if case .link(let address, _) = purpose {
                if let store = repositories.first(where: {
                    isGithubRepository(address: address, fullName: $0.fullName, sshAddress: $0.sshAddress)
                }) {
                    choose(store)
                } else {
                    stage = .notInstalled(installUrl: installUrl, storeMissing: true)
                }
            } else {
                stage = .repositories(repositories, installUrl: installUrl)
            }
        case .failure(let error):
            stage = .listFailed(GitHubWords.problem(error), signInAgain: GitHubWords.endedSignIn(error))
        }
    }

    /// The person chose `repository`: the core remembers it for this store
    /// and gives the address setup takes.
    func choose(_ repository: GitHubRepository) {
        generation += 1
        let generation = generation
        stage = .choosing(repository.fullName)
        let name = repository.fullName
        access.run({ try $0.selectRepository(fullName: name) }) { [weak self] result in
            self?.chosen(result, generation: generation)
        }
    }

    private func chosen(_ result: Result<String, Error>, generation: Int) {
        guard generation == self.generation else { return }
        switch result {
        case .success(let address):
            switch purpose {
            case .pick(let chosen): chosen(address)
            case .link(_, let linked): linked()
            }
        case .failure(let error):
            stage = .listFailed(GitHubWords.problem(error), signInAgain: GitHubWords.endedSignIn(error))
        }
    }
}
