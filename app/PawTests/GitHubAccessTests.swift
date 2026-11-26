import XCTest
@testable import Paw

/// The GitHub sign-in as the screens see it: what a call through it
/// changes (signing in, choosing the store, signing out) already shows in
/// `signedIn` and `link` when the call's answer is heard. Against the
/// core's simulated GitHub, serving the fixture's simulated git host.
@MainActor
final class GitHubAccessTests: XCTestCase {
    /// A call's answer, and what the access said as it came.
    @MainActor
    private final class Answer<Value> {
        var result: Result<Value, Error>?
        var signedIn = false
        var linked: String?

        func heard(_ result: Result<Value, Error>, _ access: GitHubAccess) {
            self.result = result
            signedIn = access.signedIn
            linked = access.link?.fullName
        }
    }

    private func call<Value>(_ access: GitHubAccess, _ what: String,
                             _ work: @escaping @Sendable (GitHubAccount) throws -> Value) async throws -> Answer<Value> {
        let answer = Answer<Value>()
        access.run(work) { [access] result in
            answer.heard(result, access)
        }
        await waitFor(what, timeout: 30) { answer.result != nil }
        _ = try XCTUnwrap(answer.result).get()
        return answer
    }

    func testWhatACallChangesIsReadAgainBeforeItsAnswer() async throws {
        let fixture = try Fixture(writable: true)
        let host = try XCTUnwrap(SimRemoteHandle.shared.host)
        let github = try SimGitHub.start(remote: host, repository: "mira/password-store")
        let stateDir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: stateDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: stateDir) }
        let access = GitHubAccess(
            core: github.openAccount(stateDir: stateDir.path, vault: SimTokenVault().asVault()), hosts: { _ in false })
        XCTAssertFalse(access.signedIn)

        let code = try await call(access, "a code") { try $0.startSignIn() }
        let signIn = try XCTUnwrap(code.result).get()
        let polled = Answer<SignInState>()
        access.waitForCode(signIn) { [access] result in
            polled.heard(result, access)
        }
        await waitFor("the sign-in", timeout: 30) { polled.result != nil }
        guard case .success(.signedIn)? = polled.result else {
            return XCTFail("The simulated GitHub signs in at the first poll, it said \(String(describing: polled.result))")
        }
        XCTAssertTrue(polled.signedIn, "Signed in by the time the answer is heard")

        _ = try await call(access, "the list") { try $0.repositories() }
        let chosen = try await call(access, "the store chosen") { try $0.selectRepository(fullName: "mira/password-store") }
        XCTAssertEqual(chosen.linked, "mira/password-store")

        let out = try await call(access, "the sign-out") { try $0.signOut(revoke: false) }
        XCTAssertFalse(out.signedIn)
        XCTAssertEqual(out.linked, "mira/password-store", "Signing out keeps the chosen repository")
        _ = fixture
    }
}
