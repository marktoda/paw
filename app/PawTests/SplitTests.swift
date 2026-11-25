import XCTest
@testable import Paw

private let a = UUID()
private let b = UUID()

/// The iPad split view's columns from the one route list (`SplitPath`),
/// and what the model does with it: one entry on the right at a time.
@MainActor
final class SplitTests: XCTestCase {
    func testATopLevelFolderStartsTheMiddleColumnAgain() {
        let split = SplitPath([.folder("web"), .folder("work"), .folder("work/old")])
        XCTAssertEqual(split.folders, [.folder("work"), .folder("work/old")])
        XCTAssertEqual(split.folder, "work/old")
        XCTAssertEqual(split.details, [])
    }

    func testAnEntryReplacesWhatWasOnTheRight() {
        let split = SplitPath([.folder("web"), .entry(a), .unpushed, .entry(b)])
        XCTAssertEqual(split.path, [.folder("web"), .entry(b)])
    }

    func testASyncScreenReplacesAnEntryButStacksOnOtherScreens() {
        XCTAssertEqual(SplitPath([.entry(a), .sync(.heldBack)]).details, [.sync(.heldBack)])
        XCTAssertEqual(SplitPath([.anotherKey("x"), .sync(.replaced)]).details, [.sync(.replaced)])
        XCTAssertEqual(
            SplitPath([.unpushed, .sync(.review(folder: "work"))]).details,
            [.unpushed, .sync(.review(folder: "work"))])
    }

    func testFoldersComeFirstWhateverTheOrderTheyArrived() {
        XCTAssertEqual(SplitPath([.unpushed, .folder("web")]).path, [.folder("web"), .unpushed])
    }

    func testPickingAFolderClosesAnEntryWhoseRowIsNoLongerShown() {
        XCTAssertEqual(SplitPath([.entry(a), .folder("web")]).path, [.folder("web")])
        XCTAssertEqual(SplitPath([.folder("web"), .entry(a), .folder("bank")]).path, [.folder("bank")])
        XCTAssertEqual(SplitPath([.folder("web"), .entry(a), .folder("web/sub")]).details, [.entry(a)])
    }

    func testNoFolderPicked() {
        XCTAssertNil(SplitPath([]).folder)
        XCTAssertNil(SplitPath([.unpushed]).folder)
    }

    func testOnlyEntriesAndTheirExplanationsShowAnEntry() {
        XCTAssertTrue(Route.entry(a).showsEntry)
        XCTAssertTrue(Route.anotherKey("x").showsEntry)
        XCTAssertFalse(Route.unpushed.showsEntry)
        XCTAssertFalse(Route.folder("web").showsEntry)
        XCTAssertFalse(Route.sync(.heldBack).showsEntry)
    }

    func testOpeningASecondEntryClosesTheOneOnTheRight() async throws {
        let fixture = try Fixture()
        let model = fixture.model
        model.setSplitView(true)
        model.path = [.folder("web")]
        model.pathChanged()

        let openedFirst = await fixture.open("web/github.com")
        let first = try XCTUnwrap(openedFirst)
        XCTAssertEqual(model.path, [.folder("web"), .entry(first)])
        XCTAssertEqual(model.splitFolder, "web")
        XCTAssertEqual(model.splitShownEntry?.ref.path, "web/github.com")
        XCTAssertEqual(model.splitShownPath, "web/github.com")

        // Tapping the entry already shown does nothing.
        model.open(EntryRef(path: "web/github.com"))
        XCTAssertNil(model.keyRequest)
        XCTAssertNotNil(model.session(first))

        // Another entry: the first closes before the key sheet.
        model.open(EntryRef(path: "web/gitlab.com"))
        XCTAssertNil(model.session(first), "The entry on the right closes first")
        XCTAssertEqual(model.path, [.folder("web")])
        await fixture.dismissDoneSheet()
        let second = try XCTUnwrap(fixture.shownEntry)
        XCTAssertEqual(model.path, [.folder("web"), .entry(second)])
        XCTAssertEqual(model.splitShownPath, "web/gitlab.com")
    }

    func testTheStackIsPutInColumnOrderWhenTheSplitViewStarts() throws {
        let model = try Fixture().model
        model.path = [.unpushed, .folder("web")]
        model.setSplitView(true)
        XCTAssertEqual(model.path, [.folder("web"), .unpushed])
        XCTAssertEqual(model.splitFolder, "web")
        model.setSplitView(false)
        XCTAssertEqual(model.splitFolder, "")
        XCTAssertNil(model.splitShownPath)
    }
}
