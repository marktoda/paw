import XCTest
@testable import Paw

/// Sending and checking with the key, on the sync model, against the
/// core's simulated git host that the writable fixture store syncs with.
@MainActor
final class SyncModelTests: XCTestCase {
    private func syncModel(_ fixture: Fixture, file: StaticString = #filePath, line: UInt = #line) throws -> SyncModel {
        try XCTUnwrap(fixture.model.storeSession.syncing, "The fixture store should sync", file: file, line: line)
    }

    /// The sheet asks for the PIN and gets it.
    private func passPin(_ sync: SyncModel, file: StaticString = #filePath, line: UInt = #line) async {
        await waitFor("the PIN, the sheet is at \(String(describing: sync.keyStage))", file: file, line: line) {
            sync.keyStage == .pin(triesLeft: 3, forgotten: false)
        }
        sync.keyTask.pin = Fixture.pin
        sync.keyTask.continueWithPin()
    }

    /// The task ends: Done, or the status page after a failure.
    private func waitForTheEnd(_ sync: SyncModel, file: StaticString = #filePath, line: UInt = #line) async {
        await waitFor("the sync to end", timeout: 30, file: file, line: line) {
            sync.running == nil && (sync.keyStage == .done || sync.sheet == nil || sync.sheet?.mode == .status)
        }
    }

    func testThePillBeforeAnyCheckAndWithAChangeWaiting() async throws {
        let fixture = try Fixture(writable: true)
        let sync = try syncModel(fixture)
        XCTAssertEqual(sync.pillState, .needsKey)
        XCTAssertFalse(sync.canSend)
        try await fixture.add("waiting.example", folder: "web")
        XCTAssertEqual(sync.pillState, .notSent(checked: nil))
        XCTAssertTrue(sync.canSend)

        // With a change waiting the pill shows the status page; with none
        // and no check yet, it goes straight to the key.
        sync.pillTapped()
        XCTAssertEqual(sync.sheet?.mode, .status)
    }

    func testThePillGoesStraightToTheKeyBeforeTheFirstCheck() throws {
        let fixture = try Fixture(writable: true)
        let sync = try syncModel(fixture)
        sync.pillTapped()
        guard case .key(.refresh)? = sync.sheet?.mode else {
            return XCTFail("The pill should start a check with the key, the sheet is \(String(describing: sync.sheet?.mode))")
        }
        XCTAssertEqual(sync.pillState, .working("Checking for changes\u{2026}"))
        sync.cancelSheet()
        XCTAssertNil(sync.sheet)
        XCTAssertEqual(sync.failure?.problem, "Checking was cancelled.")
        XCTAssertEqual(sync.pillState, .needsKey, "A cancel leaves the pill as it was")
    }

    func testTryAgainAsksForTheKeyAndOpenSettingsAndACancelDoNot() throws {
        let sync = try syncModel(try Fixture(writable: true))
        let offline = SyncFailure(
            SyncError.Fault(fault: .remote(failure: .unreachable(host: "github.com", detail: "timed out"))),
            during: .send, unpushed: 1, host: "github.com")
        XCTAssertTrue(sync.actionNeedsKey(after: offline))
        let hostKey = SyncFailure(
            SyncError.Fault(fault: .remote(failure: .hostKeyChanged(
                host: "git.example", expected: ["SHA256:aaa"], presented: "SHA256:ccc", bundled: false))),
            during: .refresh, unpushed: 0, host: "git.example")
        XCTAssertFalse(sync.actionNeedsKey(after: hostKey))
        XCTAssertFalse(sync.actionNeedsKey(after: SyncFailure.cancelled(during: .send, unpushed: 1)))
    }

    func testSendingAsksForThePinOnceEmptiesTheQueueAndASecondSendAsksForNoPin() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        let sync = try syncModel(fixture)
        try await fixture.add("sent.example", folder: "web")

        sync.start(.send)
        await passPin(sync)
        await waitForTheEnd(sync)
        XCTAssertNil(sync.failure)
        XCTAssertEqual(model.toast?.text, "Sent")
        await waitFor("the queue to empty") { sync.queue.unpushed.isEmpty }
        XCTAssertNotNil(sync.lastChecked)
        guard case .upToDate = sync.pillState else {
            return XCTFail("The pill should say up to date, it is \(sync.pillState)")
        }

        // A second send in the same run: the PIN is still held.
        try await fixture.add("again.example", folder: "web")
        model.toast = nil
        sync.start(.send)
        var askedForPin = false
        await waitFor("the second send to end", timeout: 30) {
            if case .pin? = sync.keyStage { askedForPin = true }
            return sync.running == nil && model.toast?.text == "Sent"
        }
        XCTAssertFalse(askedForPin, "The PIN is held for the run")
        await waitFor("the queue to empty") { sync.queue.unpushed.isEmpty }
    }

    func testCancellingASendLeavesTheQueueIntact() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        let sync = try syncModel(fixture)
        try await fixture.add("cancel.example", folder: "web")
        sync.start(.send)
        await waitFor("the PIN") { sync.keyStage == .pin(triesLeft: 3, forgotten: false) }
        sync.cancelSheet()
        XCTAssertNil(sync.sheet)
        XCTAssertEqual(sync.failure?.kind, .cancelled)
        XCTAssertEqual(sync.failure?.problem, "Sending was cancelled.")
        XCTAssertEqual(model.storeSession.syncing?.queue.unpushed.count, 1)
        XCTAssertEqual(sync.pillState, .notSent(checked: nil))
    }

    func testRefusedPushesAreWordedAndTheChangesStayQueued() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        let sync = try syncModel(fixture)
        SimRemoteHandle.shared.setRefusesPushes(true)
        defer { SimRemoteHandle.shared.setRefusesPushes(false) }
        try await fixture.add("refused.example", folder: "web")

        sync.start(.send)
        await passPin(sync)
        await waitForTheEnd(sync)
        let failure = try XCTUnwrap(sync.failure)
        XCTAssertEqual(failure.kind, .pushRefused)
        XCTAssertTrue(failure.problem.hasSuffix("refused your changes"), failure.problem)
        XCTAssertTrue(failure.nextStep.contains("Your change is saved on this iPhone."), failure.nextStep)
        XCTAssertEqual(sync.sheet?.mode, .status, "The sheet turns into the status page")
        XCTAssertEqual(model.storeSession.syncing?.queue.unpushed.count, 1)
        XCTAssertEqual(sync.pillState, .failed("Not sent \u{00B7} refused"))
    }

    func testCheckingNamesWhatAnotherComputerAdded() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        let sync = try syncModel(fixture)
        _ = try SimRemoteHandle.shared.apply(.add)

        sync.check()
        await passPin(sync)
        await waitFor("the summary", timeout: 30) { sync.summary != nil }
        XCTAssertNil(sync.failure)
        XCTAssertEqual(sync.summary?.added, ["web/forum.example"])
        XCTAssertEqual(sync.summary?.title, "1 password changed on your other devices")
        let web = try model.storeSession.live.listing("web").get().entries.map(\.path)
        XCTAssertTrue(web.contains("web/forum.example"), "The list reads the store again")
    }
}

private extension SyncModel {
    /// The key sheet's stage, `nil` when the sheet is not following a task.
    var keyStage: KeySheetStage? {
        if case .key? = sheet?.mode { return keyTask.stage }
        return nil
    }
}
