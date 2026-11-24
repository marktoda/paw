import XCTest
@testable import Paw

/// What AutoFill reads from a site's address and an entry's path, before
/// the core matches them.
final class SiteMatchTests: XCTestCase {
    func testTheHostOfAServiceIsLowerCasedWithoutItsTrailingDot() {
        XCTAssertEqual(SiteMatch.host(of: "https://GitHub.com/login"), "github.com")
        XCTAssertEqual(SiteMatch.host(of: "github.com/login"), "github.com")
        XCTAssertEqual(SiteMatch.host(of: "example.com.:8443"), "example.com")
        XCTAssertNil(SiteMatch.host(of: "   "))
        XCTAssertNil(SiteMatch.host(of: "https://"))
    }

    func testOnlyPlainAddressesWithADotAreMatchedAutomatically() {
        XCTAssertTrue(SiteMatch.isPlain("https://github.com/login"))
        XCTAssertFalse(SiteMatch.isPlain("http://github/"), "A dotless host anyone can register")
        XCTAssertFalse(SiteMatch.isPlain("https://p\u{0430}ypal.com"), "Lookalike letters")
        XCTAssertFalse(SiteMatch.isPlain("https://exa mple.com"))
        XCTAssertFalse(SiteMatch.isPlain(""))
    }

    func testOnlyAHostOrANameAlreadyFilledHereFillsWithoutAsking() {
        XCTAssertTrue(SiteMatch.isTrusted(.exactHost))
        XCTAssertTrue(SiteMatch.isTrusted(.sameDomain))
        XCTAssertTrue(SiteMatch.isTrusted(.name(pairing: .paired)))
        XCTAssertFalse(SiteMatch.isTrusted(.name(pairing: .unpaired)))
        XCTAssertFalse(SiteMatch.isTrusted(.name(pairing: .pairedElsewhere(sites: ["bankofamerica.co"]))))
    }

    func testTheSiteOfAnEntryIsTheLastPartThatIsAHost() {
        let named = EntrySite.domain(ofPath: "web/GitHub.com")
        XCTAssertEqual(named?.domain, "github.com")
        XCTAssertEqual(named?.isName, true)
        let user = EntrySite.domain(ofPath: "github.com/alice")
        XCTAssertEqual(user?.domain, "github.com")
        XCTAssertEqual(user?.isName, false)
        XCTAssertNil(EntrySite.domain(ofPath: "servers/10.0.0.1"), "An IP address is no site")
        XCTAssertNil(EntrySite.domain(ofPath: "GitHub"))
        XCTAssertNil(EntrySite.domain(ofPath: "web/-bad.com"))
        XCTAssertEqual(EntrySite.suggestionUser(forPath: "github.com/alice"), "alice")
        XCTAssertEqual(EntrySite.suggestionUser(forPath: "web/github.com"), "github.com")
    }
}
