import XCTest
@testable import Paw

/// One "store changed" signal: a write, wherever it is made, reaches the
/// queue, sync, the lists, the open form, Settings and AutoFill's
/// suggestions, with no caller reading anything again by hand.
@MainActor
final class StoreChangeTests: XCTestCase {
    func testAWriteReachesTheQueueSyncAndTheListsWithNoReload() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        let sync = try XCTUnwrap(model.storeSession.syncing, "The fixture store should sync")
        let queue = sync.queue
        try await fixture.add("heard.example", folder: "web")
        XCTAssertEqual(queue.unpushed.map(\.entry), ["web/heard.example"])
        sync.pillTapped()
        XCTAssertEqual(sync.sheet?.mode, .status, "Sync counts the change: the pill opens the status page")
        sync.cancelSheet()

        // Made on the queue alone, the discard still reaches them all.
        let change = try XCTUnwrap(queue.unpushed.first)
        XCTAssertTrue(queue.discard(change.id))
        XCTAssertTrue(queue.unpushed.isEmpty)
        XCTAssertFalse(model.storeSession.live.search("heard").contains { $0.entry.path == "web/heard.example" })
        sync.pillTapped()
        guard case .key(.refresh)? = sync.sheet?.mode else {
            return XCTFail("With nothing waiting the pill checks with the key, the sheet is \(String(describing: sync.sheet?.mode))")
        }
        sync.cancelSheet()
    }

    func testTheOpenFormHearsAStoreChange() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        let edits = try XCTUnwrap(model.storeSession.edits)
        let form = edits.addForm(folder: "web")
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

    func testAutoFillSuggestionsFollowOpensAndChanges() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model

        let stateDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("paw-codes-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: stateDir, withIntermediateDirectories: true)
        var published: [PassStore] = []
        var codes: OneTimeCodeEntries?
        let suggestions = AutoFillSuggestions(stateDir: stateDir.path) { request in
            if case .replace(let store, _, let used) = request {
                published.append(store)
                codes = used
            }
            return Task {}
        }
        suggestions.follow(model.storeSession.live)
        XCTAssertEqual(published.count, 1, "Written when the store opens")
        let used = try XCTUnwrap(codes)
        try used.setEnabled(enabled: true)

        try await fixture.add("suggested.example", folder: "web")
        XCTAssertEqual(published.count, 2, "And after a change")
        XCTAssertTrue(published.last === model.store, "With the store as it is now")

        await fixture.open("web/github.com")
        await waitFor("the one-time code to be noted") { published.count == 3 }
        XCTAssertEqual(used.entries(store: model.store), ["web/github.com"])
        withExtendedLifetime(suggestions) {}
    }

    /// The suggestions a background check publishes are made as the open
    /// store's are, from that store's own state folder.
    func testAPublishUsesTheStoresOwnStateFolder() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        let stateDir = try XCTUnwrap(model.storeSession.stateDir)
        let codes = OneTimeCodeEntries.open(stateDir: stateDir)
        try codes.setEnabled(enabled: true)
        _ = try codes.note(entry: "web/github.com", hasCode: true)
        var requests: [CredentialIdentities.Request] = []
        let suggestions = AutoFillSuggestions(stateDir: stateDir) { request in
            requests.append(request)
            return Task {}
        }
        await suggestions.publish(model.store).value
        guard case .replace(let store, _, let used)? = requests.last else {
            return XCTFail("A publish replaces the suggestions, it asked for \(requests)")
        }
        XCTAssertTrue(store === model.store)
        XCTAssertEqual(used.entries(store: store), ["web/github.com"], "The codes are this store's own")
    }

    /// Undo on the delete toast takes back the change the delete made, as
    /// the core returned it.
    func testUndoTakesBackTheDeletesOwnChange() async throws {
        let fixture = try Fixture(writable: true)
        let model = fixture.model
        let edits = try XCTUnwrap(model.storeSession.edits)
        let queue = try XCTUnwrap(edits.queue)
        let id = try XCTUnwrap(await fixture.open("web/github.com"))
        let session = try XCTUnwrap(model.session(id))

        edits.delete(session)
        let offer = try XCTUnwrap(edits.undoOffer)
        XCTAssertEqual(offer.change, queue.unpushed.first?.id, "The delete's own commit")
        XCTAssertNil(model.store.entry(path: "web/github.com"))

        edits.undo(offer)
        XCTAssertNotNil(model.store.entry(path: "web/github.com"), "Back")
        XCTAssertTrue(queue.unpushed.isEmpty)
        XCTAssertEqual(model.toast?.text, "github.com is back.")
    }
}
