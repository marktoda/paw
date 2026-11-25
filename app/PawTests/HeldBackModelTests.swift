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
            if case .key(_, _, .pin)? = sync.sheet?.mode { return true }
            return false
        }
        sync.pinEntry = Fixture.pin
        sync.continueWithPin()
    }

    func testAHeldBackChangeIsReencryptedOnlyWhenAskedAndCanThenBeSent() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        let edits = try XCTUnwrap(model.edits)
        let sync = try XCTUnwrap(edits.syncing, "The fixture store should sync")
        // A second key paw has the public half of, then work/ is given to both.
        let other = SimulatedKey.newOther(pin: Fixture.pin, touchRequired: false, touchTimeoutMs: 15_000)
        _ = try edits.writer.pinCardKey(connection: other.asConnection())
        try fixture.add("held.example", folder: "work")
        let host = try XCTUnwrap(SimRemoteHandle.shared.host)
        let list = fixture.key.fingerprintHex() + "\n" + other.fingerprintHex() + "\n"
        _ = try host.commitFile(path: "work/.gpg-id", contents: Data(list.utf8), message: "Give work a second key.")

        sync.check(app: model)
        await passPin(sync)
        await waitFor("the held-back change", timeout: 30) {
            sync.running == nil && sync.sheet == nil && sync.reencryptLeft["work/held.example"] != nil
        }
        XCTAssertEqual(sync.heldBack.map(\.entry), ["work/held.example"])
        XCTAssertEqual(sync.reencryptLeft["work/held.example"], .notAccepted)
        XCTAssertEqual(sync.reencryptReady, 0, "Not before the new list is accepted")
        XCTAssertFalse(sync.canSend(unpushed: 1))

        try edits.writer.acceptRecipients(folder: "work")
        sync.recomputeLocal()
        XCTAssertEqual(sync.reencryptReady, 1)
        XCTAssertEqual(sync.heldBack.count, 1, "Accepting alone does not release it")
        XCTAssertNil(sync.running, "Nothing starts by itself")

        sync.startReencrypt(app: model)
        guard case .key(.reencrypt, _, _)? = sync.sheet?.mode else {
            return XCTFail("Re-encrypt should open the key sheet, it is \(String(describing: sync.sheet?.mode))")
        }
        await waitFor("the re-encrypt to end", timeout: 30) {
            sync.running == nil && sync.sheet?.mode == .status
        }
        XCTAssertNil(sync.failure)
        XCTAssertTrue(sync.heldBack.isEmpty)
        XCTAssertEqual(model.toast, "Re-encrypted. Not sent yet.")
        XCTAssertEqual(edits.unpushed.count, 1, "Still one change, not sent")
        XCTAssertTrue(sync.canSend(unpushed: 1))

        sync.start(.send, app: model)
        await waitFor("the send", timeout: 30) { sync.running == nil && model.toast == "Sent" }
        await waitFor("the queue to empty") { edits.unpushed.isEmpty }
    }
}
