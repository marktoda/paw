import XCTest
@testable import Paw

/// One "store changed" signal: a write, wherever it is made, reaches the
/// queue, sync, the lists, the open form, Settings and AutoFill's upkeep,
/// with no caller reading anything again by hand.
@MainActor
final class StoreChangeTests: XCTestCase {
    func testAWriteReachesTheQueueSyncAndTheListsWithNoReload() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        let edits = try XCTUnwrap(model.edits)
        let sync = try XCTUnwrap(model.syncing, "The fixture store should sync")
        try await fixture.add("heard.example", folder: "web")
        XCTAssertEqual(edits.unpushed.map(\.entry), ["web/heard.example"])
        sync.pillTapped()
        XCTAssertEqual(sync.sheet?.mode, .status, "Sync counts the change: the pill opens the status page")
        sync.cancelSheet()

        // Made on the write side alone, the discard still reaches them all.
        let change = try XCTUnwrap(edits.unpushed.first)
        try edits.discard(change.id)
        XCTAssertTrue(edits.unpushed.isEmpty)
        XCTAssertFalse(model.search("heard").contains { $0.entry.path == "web/heard.example" })
        sync.pillTapped()
        guard case .key(.refresh)? = sync.sheet?.mode else {
            return XCTFail("With nothing waiting the pill checks with the key, the sheet is \(String(describing: sync.sheet?.mode))")
        }
        sync.cancelSheet()
    }

    func testTheOpenFormHearsAStoreChange() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        let edits = try XCTUnwrap(model.edits)
        let form = EntryFormModel(
            adding: "web", writer: edits.writer, folders: model.allFolders, live: model.storeSession.live)
        form.binding(\.name, field: "name").wrappedValue = "late.example"
        XCTAssertNil(form.problem(for: .name))

        try await fixture.add("late.example", folder: "web")
        XCTAssertEqual(form.problem(for: .name), "There's already an entry called 'late.example' in web.")
    }

    func testTheRemoveWarningCountsTheChangesAsTheyAreNow() async throws {
        let fixture = try Fixture(writable: true)
        let section = RemoveStoreSection(model: fixture.model)
        XCTAssertFalse(section.warning.contains("will be lost"), section.warning)
        try await fixture.add("one.example", folder: "web")
        XCTAssertTrue(section.warning.hasSuffix(" 1 change not sent yet will be lost."), section.warning)
        try await fixture.add("two.example", folder: "web")
        XCTAssertTrue(section.warning.hasSuffix(" 2 changes not sent yet will be lost."), section.warning)
    }

    func testAutoFillUpkeepFollowsOpensAndChangesAndAFixtureHasNone() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        XCTAssertNil(model.storeSession.autoFill, "A fixture's entries never reach the system's suggestions")

        let stateDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("paw-codes-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: stateDir, withIntermediateDirectories: true)
        let codes = OneTimeCodeEntries.open(stateDir: stateDir.path)
        try codes.setEnabled(enabled: true)
        var published: [PassStore] = []
        let upkeep = AutoFillUpkeep(live: model.storeSession.live, codes: codes) { published.append($0) }
        XCTAssertEqual(published.count, 1, "Written when the store opens")

        try await fixture.add("suggested.example", folder: "web")
        XCTAssertEqual(published.count, 2, "And after a change")
        XCTAssertTrue(published.last === model.store, "With the store as it is now")

        await fixture.open("web/github.com")
        await waitFor("the one-time code to be noted") { published.count == 3 }
        XCTAssertEqual(codes.entries(store: model.store), ["web/github.com"])
        withExtendedLifetime(upkeep) {}
    }
}
