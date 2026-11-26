import XCTest

@testable import Paw

/// The core sends reasons; these are the sentences it used to send for
/// each, word for word.
final class RemoteWordsTests: XCTestCase {
    func testEachAddressProblemSaysWhatToTypeInstead() {
        let said: [(AddressProblem, String)] = [
            (.empty, "Type or paste the address of your password store."),
            (.hasSpaces, "An address has no spaces in it. Check what was pasted."),
            (.notGit, "That doesn't look like a git address. It looks like git@github.com:you/password-store.git."),
            (.useSsh, "paw connects over SSH for now. Use the address that starts with git@ or ssh://."),
            (.hasSecret, "That address has a user name, password or token in it. paw logs in with your YubiKey and "
                + "needs none of them: remove that part. If it was a password or token, change it, since it has "
                + "been copied."),
            (.needsSshAddress(host: "gitlab.com"), "paw needs the SSH address, as in "
                + "git@gitlab.com:you/password-store.git. Your git host shows it beside the Clone button."),
            (.looksLikeFolder, "That looks like a folder on a computer. paw needs the address of a git server."),
            (.noUser, "Add the user name in front, as in git@github.com:you/password-store.git."),
            (.noRepository, "Add the repository after the colon, as in git@github.com:you/password-store.git."),
            (.sshWithoutUser, "Add the user name, as in ssh://git@example.com/you/password-store.git."),
            (.badPort, "The port after the colon must be a number from 1 to 65535."),
            (.sshWithoutPath,
             "Add the repository's path after the host, as in ssh://git@example.com/you/password-store.git."),
            (.notARepositoryPage, "Use the address of the repository itself, as in "
                + "https://github.com/you/password-store, or its SSH address."),
        ]
        for (problem, sentence) in said {
            XCTAssertEqual(addressProblemWords(problem), sentence)
        }
    }

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

    func testEachRefusedPathSaysWhy() {
        let said: [(RefusedReason, String)] = [
            (.tooBig(maxEntries: 100000, maxMib: 16),
             "the repository holds more than 100000 files and folders, or more than 16 MiB of names"),
            (.notUtf8, "its name is not UTF-8"),
            (.hasSlash, "its name contains a slash"),
            (.pathTooLong, "its path is too long"),
            (.unsafeName(part: .nulOrBackslash), "it contains a NUL or a backslash"),
            (.unsafeName(part: .absolute), "it is absolute"),
            (.unsafeName(part: .emptyComponent), "it has an empty component"),
            (.unsafeName(part: .dotComponent), "it has a `.` or `..` component"),
            (.unsafeName(part: .gitDirectory), "it names git's own directory"),
            (.gpgIdAlias, "some file systems would take it for .gpg-id"),
            (.gpgIdNotFile, "a .gpg-id must be a file"),
            (.nestedTooDeep, "its folders are nested too deeply"),
            (.symbolicLink, "it is a symbolic link"),
            (.submodule, "it is a submodule, a link to another repository"),
            (.notRegularFile, "it is not a regular file"),
            (.appearsTwice, "it appears twice in the same folder"),
            (.sameNameAs(first: "cafe"), "on an iPhone it is the same name as cafe"),
        ]
        for (reason, sentence) in said {
            XCTAssertEqual(refusedReasonWords(reason), sentence)
        }
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

    func testRemoteFailuresReadAsTheyDidUnderDetails() {
        let said: [(RemoteFailure, String)] = [
            (.noSavedHostKey(host: "git.example"), "git.example has no saved key; set the store up again"),
            (.hostKeyNotConfirmed(host: "git.example"),
             "git.example now shows a different key from the one you checked"),
            (.loginKeyUnusable(detail: "bad key"), "this key cannot log in to a git host: bad key"),
            (.pushRefused(detail: "git error: the host refused the push: no"),
             "git error: the host refused the push: no"),
            (.keptMoving, "the remote kept changing; try again"),
            (.notSignedIn, "not signed in to GitHub"),
            (.signedOut, "GitHub sign-in ended; sign in again"),
            (.unauthorised,
             "GitHub: GitHub did not let paw's sign-in read this repository; is paw still installed on it?"),
            (.notLinked, "no GitHub repository is chosen for this store"),
            (.wrongRepository(store: "git@github.com:you/a.git", granted: "you/b"),
             "this store is git@github.com:you/a.git, but GitHub sign-in is for you/b; paw did not use it"),
            (.gitHubRefused(message: "GitHub answered 404"), "GitHub: GitHub answered 404"),
            (.hostKeyBundled(host: "github.com"), hostKeyBundled("github.com")),
        ]
        for (failure, line) in said {
            XCTAssertEqual(remoteDetail(failure), line)
        }
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
        let words = SetupWords.words(.Fault(fault: .remote(failure: missing)), during: .downloading)
        XCTAssertEqual(words.problem, "No store at that address")
        XCTAssertEqual(
            words.nextStep,
            "github.com says you/store.git doesn't exist, or this key can't see it. Check the address and your "
                + "account's access.")
        XCTAssertEqual(words.details, ["github.com said: ERROR: Repository not found."])

        let fallback = PortFallback(fromHost: "github.com", fromPort: 22, host: "ssh.github.com", port: 443)
        let blocked = SetupWords.words(
            .Fault(fault: .remote(failure: .noConnection(host: "github.com", detail: "could not connect: timed out", fallback: fallback))),
            during: .checkingHost)
        XCTAssertEqual(blocked.problem, "Can't reach github.com")
        XCTAssertEqual(blocked.details, ["could not connect: timed out"])
        XCTAssertEqual(blocked.offer?.title, "Try port 443")
    }
}
