import XCTest
@testable import Paw

/// "Set up again" when the store can't be opened says what it would lose,
/// and never that nothing will be lost when the changes can't be counted.
@MainActor
final class StoreProblemTests: XCTestCase {
    func testAQueueThatCantBeReadMayLoseChanges() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("paw-unit-\(UUID().uuidString)", isDirectory: true)
        let repo = URL(fileURLWithPath: try WritableFixture.prepare(root: root).repoDir, isDirectory: true)
        let stateDir = repo.deletingLastPathComponent()
        let nothingLost = removeStoreWarning(unpushed: 0)

        XCTAssertEqual(StoreRootModel.unpushed(stateDir: stateDir), 0)

        try FileManager.default.removeItem(at: repo.appendingPathComponent(".git", isDirectory: true))
        let unpushed = StoreRootModel.unpushed(stateDir: stateDir)
        XCTAssertNil(unpushed, "A queue that can't be read is not an empty one")
        XCTAssertEqual(removeStoreWarning(unpushed: unpushed), nothingLost + " Changes not sent yet may be lost.")
    }

    func testNoStoreFolderCountsNothing() {
        XCTAssertNil(StoreRootModel.unpushed(stateDir: nil))
    }
}
