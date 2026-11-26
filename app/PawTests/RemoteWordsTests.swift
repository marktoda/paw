import XCTest

@testable import Paw

/// The core sends reasons; the app words them. What these check is how
/// the words are put together: names filled in, parts joined, a detail
/// made a sentence.
final class RemoteWordsTests: XCTestCase {
    func testEachNoticeSaysHowTheAddressWasRead() {
        let address = "git@github.com:you/store.git"
        let instead = "paw connects over SSH, so it will use \(address) instead."
        XCTAssertEqual(
            addressNoticeWords(.gitHubShortForm(owner: "you", repo: "store", address: address)),
            "That was GitHub's short form for you/store. \(instead)")
        XCTAssertEqual(
            addressNoticeWords(.gitHubWithoutUser(address: address)),
            "That was a GitHub address without its user name. \(instead)")
        XCTAssertEqual(addressNoticeWords(.gitHubWebAddress(address: address)), "That was a web address. \(instead)")
        XCTAssertEqual(
            addressNoticeWords(.portFallback(fallback: PortFallback(
                fromHost: "github.com", fromPort: 22, host: "ssh.github.com", port: 443))),
            "Port 22 of github.com could not be reached, so paw will use ssh.github.com on port 443 instead.")
    }

    func testAPathRefusedForTwoReasonsGivesBoth() {
        let both = RefusedPath(path: "caf\u{e9}", reasons: [.symbolicLink, .sameNameAs(first: "cafe")])
        XCTAssertEqual(refusedWords(both), "it is a symbolic link, and on an iPhone it is the same name as cafe")
    }

    func testARefusedRemoteNamesItsFirstFivePaths() {
        let paths = (1...7).map { RefusedPath(path: "p\($0)", reasons: [.symbolicLink]) }
        XCTAssertEqual(
            refusedMessage(Array(paths.prefix(1))),
            "the remote contains something paw will not check out: p1 (it is a symbolic link)")
        XCTAssertEqual(
            refusedMessage(paths),
            "the remote contains something paw will not check out: "
                + (1...5).map { "p\($0) (it is a symbolic link)" }.joined(separator: "; ") + "; and 2 more")
    }

    func testARemoteFailureUnderDetailsBecomesASentence() {
        XCTAssertEqual(
            remoteDetail(.noSavedHostKey(host: "git.example")), "git.example has no saved key; set the store up again")
        XCTAssertEqual(
            SyncError.Fault(fault: .remote(failure: .noSavedHostKey(host: "git.example"))).sentence,
            "Git.example has no saved key; set the store up again.")
    }

    func testQueueRefusalsReadAsTheyDid() {
        XCTAssertEqual(
            SyncError.BeingSent.sentence, "This change is being sent right now; wait for the send to finish.")
        XCTAssertEqual(SyncError.NotWaiting(id: "abc123").sentence, "Abc123 is not an unpushed change.")
        XCTAssertEqual(
            SyncError.QueueKeptChanging.sentence,
            "The waiting changes kept changing while paw was re-encrypting them; try again.")
        let failure = SyncFailure(SyncError.QueueKeptChanging, during: .reencrypt, unpushed: 0, host: "h")
        XCTAssertEqual(failure.details, ["the waiting changes kept changing while paw was re-encrypting them; try again"])
    }

    func testGitHubSaysAnUnacceptedSignInAndAnUnlistedRepositoryAsBefore() {
        XCTAssertEqual(
            GitHubWords.problem(GitHubError.Fault(fault: .remote(failure: .unauthorised))),
            "GitHub didn't allow it: GitHub did not accept paw's sign-in. Try again.")
        XCTAssertEqual(
            GitHubWords.problem(GitHubError.NotListed(fullName: "you/store")),
            "GitHub didn't allow it: you/store is not in the list; list the repositories again. Try again.")
        XCTAssertEqual(
            GitHubWords.problem(GitHubError.Fault(fault: .remote(failure: .unreachable(host: "github.com", detail: "offline")))),
            "Couldn't reach GitHub. Check your connection and try again.")
        XCTAssertTrue(GitHubWords.endedSignIn(GitHubError.Fault(fault: .remote(failure: .signedOut))))
        XCTAssertFalse(GitHubWords.endedSignIn(GitHubError.Fault(fault: .remote(failure: .notLinked))))
    }

    func testSetupSaysNoStoreAtThatAddressWithWhatTheHostSaid() {
        let missing = RemoteFailure.repositoryMissing(
            host: "github.com", repository: "you/store.git", said: "ERROR: Repository not found.", detail: "git error")
        let words = SetupWords.words(.Fault(fault: .remote(failure: missing)), downloading: true)
        XCTAssertEqual(words.problem, "No store at that address")
        XCTAssertEqual(
            words.nextStep,
            "github.com says you/store.git doesn't exist, or this key can't see it. Check the address and your "
                + "account's access.")
        XCTAssertEqual(words.details, ["github.com said: ERROR: Repository not found."])

        let fallback = PortFallback(fromHost: "github.com", fromPort: 22, host: "ssh.github.com", port: 443)
        let blocked = SetupWords.words(
            .Fault(fault: .remote(failure: .noConnection(host: "github.com", detail: "could not connect: timed out", fallback: fallback))),
            downloading: false)
        XCTAssertEqual(blocked.problem, "Can't reach github.com")
        XCTAssertEqual(blocked.details, ["could not connect: timed out"])
        XCTAssertEqual(blocked.offer?.title, "Try port 443")
    }
}
