import Foundation
import XCTest
@testable import Paw

/// The add form answers "already used" and "can paw save here" without
/// asking the writer on each keystroke: the writer waits for the store's
/// lock, and the form runs on the main thread.
@MainActor
final class EntryFormStoreTests: XCTestCase {
    func testTypingReadsNothingFromTheStoreAndATypedFolderIsReadOnce() async throws {
        let fixture = try Fixture(writable: true)
        let edits = try XCTUnwrap(fixture.model.edits)
        let writer = CountingWriter(edits.writer)
        let folders = fixture.model.allFolders
        let form = EntryFormModel(adding: "", writer: writer, folders: folders, store: fixture.model.store)

        await waitFor("the folders' keys") { form.folderOpenable.count == folders.count }
        XCTAssertEqual(form.folderOpenable["web"], true)
        form.chooseFolder("web")
        await waitFor("who can open web") { form.recipients != nil }
        let before = writer.counts

        let name = form.binding(\.name, field: "name")
        for typed in ["G", "Gi", "Git", "GitH", "GitHu", "GitHub", "GitHub.", "GitHub.c", "GitHub.co", "GitHub.com"] {
            name.wrappedValue = typed
        }
        XCTAssertEqual(form.problem(for: .name), "There's already an entry called 'github.com' in web.")
        XCTAssertFalse(form.canSave)
        name.wrappedValue = "github.com-2"
        XCTAssertNil(form.problem(for: .name))
        XCTAssertTrue(form.canSave)
        XCTAssertEqual(writer.counts, before, "Typing a name should read nothing from the store")
        XCTAssertEqual(writer.counts.validate, 0)

        // A new folder, typed: who can open it is read once, after the pause.
        form.chooseNewFolder()
        for typed in ["s", "sh", "sho", "shop"] {
            form.newFolder = typed
            form.changed()
        }
        await waitFor("who can open shop") { form.recipients != nil }
        XCTAssertEqual(form.effectiveFolder, "shop")
        XCTAssertEqual(writer.counts.recipients, before.recipients + 1)
    }

    func testSaveWaitsForANewFolderAndKeepsItsAnswerWhenTheStoreIsReadAgain() async throws {
        let fixture = try Fixture(writable: true)
        let edits = try XCTUnwrap(fixture.model.edits)
        let form = EntryFormModel(
            adding: "web", writer: edits.writer, folders: fixture.model.allFolders, store: fixture.model.store)
        form.binding(\.name, field: "name").wrappedValue = "new.example"
        form.binding(\.password, field: "password").wrappedValue = "Typed-pass-42"
        await waitFor("who can open web") { form.folderWritable != nil }
        XCTAssertTrue(form.canSave)

        // Another folder: no answer, so no Save, until its own lands.
        form.chooseFolder("email")
        XCTAssertNil(form.recipients)
        XCTAssertFalse(form.canSave)
        await waitFor("who can open email") { form.folderWritable != nil }
        XCTAssertTrue(form.canSave)

        // The store read again: the answer for email stays while it is asked again.
        try await fixture.add("other.example", folder: "web")
        form.storeChanged(fixture.model.store)
        XCTAssertNotNil(form.recipients)
        XCTAssertTrue(form.canSave)
    }
}

/// The fixture's writer, counting the calls that read the store under its lock.
private final class CountingWriter: StoreWriter {
    struct Counts: Equatable {
        var validate = 0
        var recipients = 0
    }

    private let real: StoreWriter
    private let lock = NSLock()
    private var counted = Counts()

    init(_ real: StoreWriter) {
        self.real = real
        super.init(noHandle: StoreWriter.NoHandle())
    }

    required init(unsafeFromHandle handle: UInt64) {
        fatalError("Made only from a real writer")
    }

    var counts: Counts {
        lock.lock()
        defer { lock.unlock() }
        return counted
    }

    private func count(_ change: (inout Counts) -> Void) {
        lock.lock()
        change(&counted)
        lock.unlock()
    }

    override func newDraft(folder: String) -> EntryDraft {
        real.newDraft(folder: folder)
    }

    override func validate(draft: EntryDraft) throws -> [DraftProblem] {
        count { $0.validate += 1 }
        return try real.validate(draft: draft)
    }

    override func folderRecipients(folder: String) throws -> FolderRecipients {
        count { $0.recipients += 1 }
        return try real.folderRecipients(folder: folder)
    }
}
