import UIKit
import XCTest

/// The iPad split view on the fixture store. Skipped on an iPhone: run on
/// an iPad simulator, held in landscape so all three columns are up.
final class IPadTests: PawUITestCase {
    override func setUpWithError() throws {
        try super.setUpWithError()
        try XCTSkipUnless(UIDevice.current.userInterfaceIdiom == .pad, "iPad only")
    }

    override func tearDown() {
        super.tearDown()
        XCUIDevice.shared.orientation = .portrait
    }

    private func launch() {
        XCUIDevice.shared.orientation = .landscapeLeft
        launchFixture()
    }

    func testTheSplitViewShowsFoldersEntriesAndTheOpenEntry() {
        launch()
        expect("split-no-folder")
        expect("split-no-entry")
        snapshot("split-empty")

        tap("folder-web")
        expect("entry-github.com")
        expectGone("split-no-folder")
        expect("home-title")

        openGitHubWithKey()
        expectGone("split-no-entry")
        // Home and the folder stay beside the entry.
        expect("home-title")
        expect("entry-gitlab.com")
        snapshot("split-open")
    }

    func testOpeningASecondEntryClosesTheFirst() {
        launch()
        openGitHubWithKey()
        app.buttons["password-reveal"].tap()
        expectValue("password-value", Self.githubPassword)

        tap("entry-gitlab.com")
        expectLabel("key-sheet-purpose", contains: "Open gitlab.com")
        touchTheKey()
        expectLabel("entry-title", contains: "gitlab.com")
        expectGone("key-sheet-purpose")

        let titles = app.descendants(matching: .any).matching(identifier: "entry-title")
        XCTAssertEqual(titles.count, 1, "Only one entry should be open")
        let first = NSPredicate(
            format: "label CONTAINS %@ OR value CONTAINS %@", Self.githubPassword, Self.githubPassword)
        XCTAssertEqual(
            app.descendants(matching: .any).matching(first).count, 0,
            "Nothing of github.com's password should be left on screen")
        snapshot("second-entry")
    }

    func testCommandFPutsTheCursorInSearch() {
        launch()
        let field = app.searchFields.firstMatch
        XCTAssertTrue(waitToExist(field, timeout: Self.patience), "The middle column should have a search field")
        XCTAssertFalse(hasFocus(field), "Search starts without the cursor")
        app.typeKey("f", modifierFlags: .command)
        XCTAssertTrue(waitUntil(timeout: Self.patience) { hasFocus(field) }, "⌘F should put the cursor in search")
        field.typeText("ycombinator")
        expectLabel("search-count", contains: "1 match")
    }
}
