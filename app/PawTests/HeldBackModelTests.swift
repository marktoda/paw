import XCTest
@testable import Paw

/// Re-encrypting held-back changes by hand, on the sync model: another
/// computer gives `work/` a second key, the change saved there is held back,
/// and once the new list is accepted "Re-encrypt" opens it with the key and
/// it can be sent. `HeldBackTests` clicks through the same on the simulator.
@MainActor
final class HeldBackModelTests: XCTestCase {
    private func passPin(_ sync: SyncModel, file: StaticString = #filePath, line: UInt = #line) async {
        await waitFor("the PIN", file: file, line: line) {
            if case .key? = sync.sheet?.mode, case .pin = sync.keyTask.stage { return true }
            return false
        }
        sync.keyTask.pin = Fixture.pin
        sync.keyTask.continueWithPin()
    }

    /// A change saved in `work/`, then held back: another computer gave
    /// `work/` a second key paw has the public half of, and a check with
    /// the key brought that in.
    private func heldBackChange() async throws -> (Fixture, StoreEdits, SyncModel) {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        let edits = try XCTUnwrap(model.storeSession.edits)
        let sync = try XCTUnwrap(model.storeSession.syncing, "The fixture store should sync")
        let other = SimulatedKey.newOther(pin: Fixture.pin, touchRequired: false, touchTimeoutMs: 15_000)
        _ = try other.pinAsCard(stateDir: edits.stateDir)
        try await fixture.add("held.example", folder: "work")
        let host = try XCTUnwrap(SimRemoteHandle.shared.host)
        let list = fixture.key.fingerprintHex() + "\n" + other.fingerprintHex() + "\n"
        _ = try host.commitFile(path: "work/.gpg-id", contents: Data(list.utf8), message: "Give work a second key.")

        sync.check()
        await passPin(sync)
        await waitFor("the held-back change", timeout: 30) {
            sync.running == nil && sync.sheet == nil && sync.reencryptLeft["work/held.example"] != nil
        }
        return (fixture, edits, sync)
    }

    func testAHeldBackChangeIsReencryptedOnlyWhenAskedAndCanThenBeSent() async throws {
        let (fixture, edits, sync) = try await heldBackChange()
        let model = fixture.model
        XCTAssertEqual(sync.heldBack.map(\.entry), ["work/held.example"])
        XCTAssertEqual(sync.reencryptLeft["work/held.example"], .notAccepted)
        XCTAssertEqual(sync.reencryptReady, 0, "Not before the new list is accepted")
        XCTAssertFalse(sync.canSend)

        try edits.acceptRecipients(folder: "work")
        XCTAssertEqual(sync.reencryptReady, 1)
        XCTAssertEqual(sync.heldBack.count, 1, "Accepting alone does not release it")
        XCTAssertNil(sync.running, "Nothing starts by itself")

        sync.startReencrypt()
        guard case .key(.reencrypt)? = sync.sheet?.mode else {
            return XCTFail("Re-encrypt should open the key sheet, it is \(String(describing: sync.sheet?.mode))")
        }
        await waitFor("the re-encrypt to end", timeout: 30) {
            sync.running == nil && sync.sheet?.mode == .status
        }
        XCTAssertNil(sync.failure)
        XCTAssertTrue(sync.heldBack.isEmpty)
        XCTAssertEqual(model.toast?.text, "Re-encrypted. Not sent yet.")
        XCTAssertEqual(sync.queue.unpushed.count, 1, "Still one change, not sent")
        XCTAssertTrue(sync.canSend)

        sync.start(.send)
        await waitFor("the send", timeout: 30) { sync.running == nil && model.toast?.text == "Sent" }
        await waitFor("the queue to empty") { sync.queue.unpushed.isEmpty }
    }

    /// A change made while a check is running is heard: once the check
    /// ends, what sync offers follows the store as it is now, with nothing
    /// read again by hand.
    func testAChangeMadeWhileACheckRunsCountsOnceItEnds() async throws {
        let (_, edits, sync) = try await heldBackChange()
        XCTAssertEqual(sync.reencryptReady, 0)
        sync.check()
        await waitFor("the check to start") { sync.running == .refresh }
        try edits.acceptRecipients(folder: "work")
        sync.cancelSheet()
        XCTAssertNil(sync.running)
        XCTAssertEqual(sync.reencryptReady, 1, "The new list was heard during the check")
        XCTAssertEqual(sync.pillState, .paused("1 change held back \u{00B7} Review"))
    }

    /// Accepting a folder's new list reaches an open form, which asks again
    /// who can open the folder: Save follows with no call from the sheet.
    func testAnOpenFormHearsANewListAccepted() async throws {
        let (_, edits, _) = try await heldBackChange()
        let form = edits.addForm(folder: "work")
        await waitFor("who can open work") { form.folderWritable != nil }
        XCTAssertEqual(form.folderWritable, false, "Not before the new list is accepted")
        try edits.acceptRecipients(folder: "work")
        await waitFor("who can open work, read again") { form.folderWritable == true }
    }
}
