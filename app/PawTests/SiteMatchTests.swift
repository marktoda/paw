import XCTest
@testable import Paw

/// What a suggestion above the keyboard shows for an entry's path.
final class SiteMatchTests: XCTestCase {
    func testTheSuggestionUserIsTheEntrysName() {
        XCTAssertEqual(EntrySite.suggestionUser(forPath: "github.com/alice"), "alice")
        XCTAssertEqual(EntrySite.suggestionUser(forPath: "web/github.com"), "github.com")
    }
}
