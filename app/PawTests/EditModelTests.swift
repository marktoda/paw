import XCTest
@testable import Paw

/// Adding and the unpushed queue, on the model and the form's model, on
/// the writable fixture store. None of it needs the key.
@MainActor
final class EditModelTests: XCTestCase {
    func testTheCoreSaysWhatIsWrongWithANameAndSaveWaits() async throws {
        let fixture = try Fixture(writable: true)
        let edits = try XCTUnwrap(fixture.model.storeSession.edits)
        let form = edits.addForm(folder: "")

        form.changed()
        XCTAssertEqual(form.problem(for: .name), "Give it a name.")
        XCTAssertFalse(form.canSave)

        form.name = "work/vpn"
        form.changed()
        XCTAssertEqual(form.problem(for: .name), "Names can't contain /.")
        XCTAssertFalse(form.canSave)

        form.chooseFolder("web")
        form.name = "GitHub.com"
        form.changed()
        XCTAssertEqual(form.problem(for: .name), "There's already an entry called 'github.com' in web.")
        XCTAssertFalse(form.canSave)

        form.name = "github.com-2"
        form.changed()
        XCTAssertNil(form.problem(for: .name))
        await waitFor("who can open web") { form.folderWritable != nil }
        XCTAssertTrue(form.canSave)
    }

    func testTheStrengthFollowsTheLength() throws {
        let fixture = try Fixture(writable: true)
        let edits = try XCTUnwrap(fixture.model.storeSession.edits)
        let form = EntryFormModel(adding: "", writer: edits.writer, folders: [], live: fixture.model.storeSession.live)
        XCTAssertEqual(form.strengthWords, "Very strong")
        form.policy.length = 8
        form.policyChanged()
        XCTAssertEqual(form.strengthWords, "Fair")
        form.generate()
        XCTAssertEqual(form.password.count, 8)
        // A generated password follows the length.
        form.policy.length = 12
        form.policyChanged()
        XCTAssertEqual(form.password.count, 12)
    }

    func testSavingANewEntryQueuesItAndKeepsAFaceIdCopy() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        try await fixture.add("shop.example", folder: "web")
        XCTAssertEqual(model.toast?.text, "Saved. Not sent yet.")
        XCTAssertEqual(model.storeSession.syncing?.queue.unpushed.count, 1)
        XCTAssertEqual(model.storeSession.syncing?.queue.bannerTitle, "1 change not sent")
        XCTAssertEqual(model.rowAccess("web/shop.example"), .faceID, "What was just typed opens with Face ID")
        XCTAssertNil(model.keyRequest, "Saving never asks for the key")
        XCTAssertTrue(model.storeSession.live.search("shop").contains { $0.entry.path == "web/shop.example" })
    }

    func testDiscardingOneChangeTakesBackOnlyThatChange() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        try await fixture.add("keep.example", folder: "web")
        try await fixture.add("drop.example", folder: "web")
        let edits = try XCTUnwrap(model.storeSession.edits)
        let queue = try XCTUnwrap(edits.queue)
        XCTAssertEqual(queue.bannerTitle, "2 changes not sent")
        let change = try XCTUnwrap(queue.unpushed.first { $0.entry == "web/keep.example" })

        queue.discard(change.id)
        XCTAssertEqual(queue.unpushed.map(\.entry), ["web/drop.example"])
        let web = try model.storeSession.live.listing("web").get().entries.map(\.path)
        XCTAssertTrue(web.contains("web/drop.example"))
        XCTAssertFalse(web.contains("web/keep.example"), "A discarded add takes the entry away")

        queue.discardAll()
        XCTAssertEqual(queue.unpushed.count, 0)
    }
}
