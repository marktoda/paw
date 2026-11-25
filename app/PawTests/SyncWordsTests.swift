import XCTest
@testable import Paw

/// How sync is worded: a failed send or check, the pill, what a sync
/// brought in, and the last background check.
final class SyncWordsTests: XCTestCase {
    // MARK: Failures

    func testWaitingChangesAreCountedOnlyWhenThereAreSome() {
        XCTAssertEqual(SyncFailure.waiting(0), "")
        XCTAssertEqual(SyncFailure.waiting(1), "Your change is saved on this iPhone.")
        XCTAssertEqual(SyncFailure.waiting(3), "Your 3 changes are saved on this iPhone.")
        XCTAssertEqual(SyncFailure.sentences("Try again.", "", "Later."), "Try again. Later.")
    }

    func testNoNetworkIsOfflineNotAnError() {
        let failure = SyncFailure(.Network(message: "timed out"), during: .send, unpushed: 1, host: "github.com")
        XCTAssertEqual(failure.kind, .offline)
        XCTAssertEqual(failure.problem, "Can't reach github.com")
        XCTAssertEqual(failure.nextStep, "Check your connection. Your change is saved on this iPhone.")
        XCTAssertEqual(failure.details, ["timed out"])
        XCTAssertEqual(failure.pill, "Offline")
        XCTAssertEqual(failure.actionTitle, "Try again")
        XCTAssertTrue(failure.actionNeedsKey)
    }

    func testAChangedHostKeyStopsSyncAndPointsToSettings() {
        let failure = SyncFailure(
            .HostKeyChanged(host: "git.example", expected: ["SHA256:aaa", "SHA256:bbb"], presented: "SHA256:ccc"),
            during: .refresh, unpushed: 0, host: "git.example")
        XCTAssertEqual(failure.kind, .hostKeyChanged)
        XCTAssertEqual(failure.problem, "git.example's identity changed")
        XCTAssertEqual(failure.details, ["Expected SHA256:aaa or SHA256:bbb", "It presented SHA256:ccc"])
        XCTAssertEqual(failure.pill, "Sync stopped \u{00B7} Review")
        XCTAssertEqual(failure.actionTitle, "Open Settings")
        XCTAssertFalse(failure.actionNeedsKey)
    }

    func testARejectedKeySaysWhatWasNotDone() {
        let check = SyncFailure(.KeyRejected, during: .refresh, unpushed: 0, host: "github.com")
        XCTAssertEqual(check.kind, .keyNotAccepted)
        XCTAssertEqual(check.problem, "github.com didn't accept this key")
        XCTAssertEqual(check.nextStep, "Check its SSH key is on your account there.")
        XCTAssertEqual(check.pill, "Not checked \u{00B7} key not accepted")
        let send = SyncFailure(.KeyRejected, during: .send, unpushed: 2, host: "github.com")
        XCTAssertEqual(send.pill, "Not sent \u{00B7} key not accepted")
        XCTAssertEqual(
            send.nextStep, "Check its SSH key is on your account there. Your 2 changes are saved on this iPhone.")
    }

    func testTheHostsMessageIsReadForARefusedPushAMissingStoreAndARefusedRemote() {
        let refused = SyncFailure(
            .Failed(message: "The host refused the push: protected branch"), during: .send, unpushed: 2, host: "h")
        XCTAssertEqual(refused.kind, .pushRefused)
        XCTAssertEqual(refused.problem, "h refused your changes")
        XCTAssertEqual(refused.nextStep, "Your 2 changes are saved on this iPhone. Details say why.")
        XCTAssertEqual(refused.pill, "Not sent \u{00B7} refused")

        let gone = SyncFailure(.Failed(message: "Repository not found"), during: .send, unpushed: 0, host: "h")
        XCTAssertEqual(gone.kind, .repositoryGone)
        XCTAssertEqual(gone.pill, "Not sent \u{00B7} store not found")

        let remote = SyncFailure(
            .Failed(message: "The remote contains something paw will not check out: Web/github.com"),
            during: .refresh, unpushed: 1, host: "h")
        XCTAssertEqual(remote.kind, .refusedRemote)
        XCTAssertEqual(remote.problem, "Your store has names paw can't download")
        XCTAssertEqual(remote.details, [])
        XCTAssertEqual(remote.pill, "Sync paused \u{00B7} Review")
        XCTAssertEqual(remote.actionTitle, "Show names")

        let other = SyncFailure(.Failed(message: "boom"), during: .refresh, unpushed: 0, host: "h")
        XCTAssertEqual(other.kind, .other)
        XCTAssertEqual(other.problem, "Couldn't check for changes")
        XCTAssertEqual(other.nextStep, "Try again.")
        XCTAssertEqual(other.details, ["boom"])
    }

    func testACancelIsQuietAndOffersNothing() {
        let send = SyncFailure.cancelled(during: .send, unpushed: 1)
        XCTAssertEqual(send.kind, .cancelled)
        XCTAssertEqual(send.problem, "Sending was cancelled.")
        XCTAssertEqual(send.pill, "")
        XCTAssertNil(send.actionTitle)
        XCTAssertFalse(send.actionNeedsKey)
        XCTAssertEqual(SyncFailure.cancelled(during: .refresh, unpushed: 0).problem, "Checking was cancelled.")
    }

    // MARK: The pill

    func testThePillSaysWhenItLastChecked() {
        let now = Date()
        XCTAssertEqual(SyncPillState.upToDate(checked: now.addingTimeInterval(-20)).text(now: now),
                       "Up to date \u{00B7} checked just now")
        XCTAssertEqual(SyncPillState.offline(checked: now.addingTimeInterval(-150)).text(now: now),
                       "Offline \u{00B7} last checked 2 min ago")
        XCTAssertEqual(SyncPillState.offline(checked: nil).text(now: now), "Offline")
        XCTAssertEqual(SyncPillState.notSent(checked: nil).text(now: now), "Not checked yet")
        XCTAssertEqual(SyncPillState.notSent(checked: now.addingTimeInterval(-600)).text(now: now), "Checked 10 min ago")
        XCTAssertEqual(SyncPillState.needsKey.text(now: now), "Updates need your key")
        XCTAssertEqual(SyncPillState.paused("Sync paused \u{00B7} Review").text(now: now), "Sync paused \u{00B7} Review")
    }

    func testThePillsLookFollowsItsState() {
        XCTAssertEqual(SyncPillState.working("Sending").status, .syncing)
        XCTAssertEqual(SyncPillState.upToDate(checked: Date()).status, .synced)
        XCTAssertEqual(SyncPillState.needsKey.status, .needsKey)
        XCTAssertEqual(SyncPillState.notChecked.status, .syncing)
        XCTAssertEqual(SyncPillState.notSent(checked: nil).status, .checked)
        XCTAssertEqual(SyncPillState.offline(checked: nil).status, .offline)
        XCTAssertEqual(SyncPillState.failed("Not sent").status, .failed)
        XCTAssertEqual(SyncPillState.paused("Sync paused").status, .failed)
    }

    func testAgoInMinutes() {
        let now = Date()
        XCTAssertEqual(syncAgo(now.addingTimeInterval(-59), now: now), "just now")
        XCTAssertEqual(syncAgo(now.addingTimeInterval(-60), now: now), "1 min ago")
        XCTAssertEqual(syncAgo(now.addingTimeInterval(-3_599), now: now), "59 min ago")
    }

    // MARK: What a sync brought in

    private func report(
        keptBoth: [KeptBoth] = [], keptLocal: [String] = [], recipients: [String] = [],
        added: [String] = [], changed: [String] = [], removed: [String] = []
    ) -> IntegrateReport {
        IntegrateReport(
            fastForwarded: 1, replayed: 0, keptBoth: keptBoth, keptLocal: keptLocal, droppedDeletes: [],
            recipientsChanged: recipients.map { RecipientsChange(path: $0, before: [], after: ["ABCD"]) },
            added: added, changed: changed, removed: removed)
    }

    func testTheSummaryCountsAddedChangedAndRemovedEntries() {
        let summary = SyncSummary(
            origin: .sync, report: report(added: ["web/d"], changed: ["web/b"], removed: ["web/c"]))
        XCTAssertEqual(summary.added, ["web/d"])
        XCTAssertEqual(summary.changed, ["web/b"])
        XCTAssertEqual(summary.removed, ["web/c"])
        XCTAssertEqual(summary.title, "3 passwords changed on your other devices")
        XCTAssertFalse(summary.needsALook)
        XCTAssertFalse(summary.isEmpty)

        let one = SyncSummary(origin: .replaced, report: report(added: ["web/e"]))
        XCTAssertEqual(one.title, "1 password updated from the new store")
    }

    func testEntriesTheReportExplainsAreNotCountedAgain() {
        let summary = SyncSummary(
            origin: .sync,
            report: report(
                keptBoth: [KeptBoth(entry: "web/example.com", copy: "web/example.com (from this iPhone)")],
                added: ["web/example.com (from this iPhone)"], changed: ["web/example.com"]))
        XCTAssertEqual(summary.entryCount, 0)
        XCTAssertEqual(summary.title, "Two versions of example.com were kept")
        XCTAssertTrue(summary.needsALook)
    }

    func testChangedKeysAreSaidByFolder() {
        let work = SyncSummary(origin: .sync, report: report(recipients: ["work/.gpg-id"]))
        XCTAssertEqual(work.recipientsChanged, ["work"])
        XCTAssertEqual(work.title, "Who can open work changed")
        let top = SyncSummary(origin: .sync, report: report(recipients: [".gpg-id"]))
        XCTAssertEqual(top.title, "Who can open your store changed")
        let empty = SyncSummary(origin: .sync, report: report())
        XCTAssertTrue(empty.isEmpty)
        XCTAssertEqual(empty.title, "Your store was updated")
    }

    // MARK: The background check

    private func background(_ outcome: BackgroundOutcome, integration: IntegrateReport? = nil,
                            decisions: [RefreshDecision] = []) -> BackgroundRun {
        BackgroundRefresh.summary(
            of: BackgroundRefreshReport(outcome: outcome, integration: integration, decisions: decisions, unpushed: 0),
            at: Date(timeIntervalSince1970: 0))
    }

    func testTheBackgroundCheckIsSaidInAFewWords() {
        let updated = background(.updated, integration: report())
        XCTAssertEqual(updated.result, "Brought in changes")
        XCTAssertTrue(updated.changed)
        XCTAssertTrue(updated.success)
        XCTAssertFalse(updated.needsLook)

        XCTAssertEqual(background(.upToDate).result, "Up to date")
        XCTAssertEqual(background(.notSignedIn).result, "Not signed in to GitHub")
        XCTAssertEqual(background(.signedOut).result, "GitHub sign-in ended")

        let offline = background(.offline(message: "no route"))
        XCTAssertEqual(offline.result, "Couldn't reach GitHub")
        XCTAssertFalse(offline.success)
        XCTAssertEqual(background(.outOfTime).result, "Ran out of time")
    }

    func testOnlyADecisionThatIsNotAHeldBackChangeNeedsThePerson() {
        let held = background(.needsDecision, decisions: [.heldBack(count: 2)])
        XCTAssertFalse(held.needsLook)
        XCTAssertEqual(held.result, "Up to date")

        let refused = background(.needsDecision, decisions: [.refusedRemote(paths: [])])
        XCTAssertTrue(refused.needsLook)
        XCTAssertEqual(refused.result, "Paused")
    }
}
