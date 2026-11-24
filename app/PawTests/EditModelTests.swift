import XCTest
@testable import Paw

/// Adding and the unpushed queue, on the model and the form's model, on
/// the writable fixture store. None of it needs the key.
@MainActor
final class EditModelTests: XCTestCase {
    func testTheCoreSaysWhatIsWrongWithANameAndSaveWaits() throws {
        let fixture = try Fixture(writable: true)
        let edits = try XCTUnwrap(fixture.model.edits)
        let form = EntryFormModel(adding: "", writer: edits.writer, folders: fixture.model.allFolders)

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
        XCTAssertTrue(form.canSave)
    }

    func testTheStrengthFollowsTheLength() throws {
        let fixture = try Fixture(writable: true)
        let edits = try XCTUnwrap(fixture.model.edits)
        let form = EntryFormModel(adding: "", writer: edits.writer, folders: [])
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

    func testSavingANewEntryQueuesItAndKeepsAFaceIdCopy() throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        try fixture.add("shop.example", folder: "web")
        XCTAssertEqual(model.toast, "Saved. Not sent yet.")
        XCTAssertEqual(model.edits?.unpushed.count, 1)
        XCTAssertEqual(model.edits?.bannerTitle, "1 change not sent")
        XCTAssertEqual(model.rowAccess("web/shop.example"), .faceID, "What was just typed opens with Face ID")
        XCTAssertNil(model.keyRequest, "Saving never asks for the key")
        XCTAssertTrue(model.search("shop").contains { $0.entry.path == "web/shop.example" })
    }

    func testDiscardingOneChangeTakesBackOnlyThatChange() throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        try fixture.add("keep.example", folder: "web")
        try fixture.add("drop.example", folder: "web")
        let edits = try XCTUnwrap(model.edits)
        XCTAssertEqual(edits.bannerTitle, "2 changes not sent")
        let change = try XCTUnwrap(edits.unpushed.first { $0.entry == "web/keep.example" })

        model.discardChange(change.id)
        XCTAssertEqual(edits.unpushed.map(\.entry), ["web/drop.example"])
        let web = try model.listing("web").get().entries.map(\.path)
        XCTAssertTrue(web.contains("web/drop.example"))
        XCTAssertFalse(web.contains("web/keep.example"), "A discarded add takes the entry away")

        model.discardAllChanges()
        XCTAssertEqual(edits.unpushed.count, 0)
    }
}
