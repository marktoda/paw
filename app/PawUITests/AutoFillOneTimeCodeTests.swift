import CryptoKit
import XCTest

/// One-time codes in the AutoFill provider, hosted in the app by the debug
/// harness: `code:` before a site (`-pawHostProvider code:https://…`) or a
/// suggestion (`-pawHostProviderEntry code:web/github.com`) makes it a
/// one-time-code request. The harness shows a filled code by its length
/// and the start of its SHA-256, which the test works out itself from the
/// fixture's setup key. `harness-codes` lists the entries paw knows have a
/// code. "Suggest one-time codes" is off unless `-pawSimOneTimeCodes 1`
/// turns it on, as Settings does. The real code field, the key icon and
/// the bar above the keyboard are checked by hand on a device.
final class AutoFillOneTimeCodeTests: PawUITestCase {
    /// The fixture web/github.com's setup key, JBSWY3DPEHPK3PXP, decoded.
    private static let secret: [UInt8] = [0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x21, 0xDE, 0xAD, 0xBE, 0xEF]

    /// What the harness reports for the fixture's code at `date` (SHA-1, 6
    /// digits, 30 seconds: RFC 6238).
    private static func codeFill(at date: Date) -> String {
        let counter = UInt64(date.timeIntervalSince1970 / 30)
        let message = withUnsafeBytes(of: counter.bigEndian) { Data($0) }
        let mac = Array(HMAC<Insecure.SHA1>.authenticationCode(for: message, using: SymmetricKey(data: Data(secret))))
        let offset = Int(mac[mac.count - 1] & 0x0F)
        let value = (UInt32(mac[offset] & 0x7F) << 24) | (UInt32(mac[offset + 1]) << 16)
            | (UInt32(mac[offset + 2]) << 8) | UInt32(mac[offset + 3])
        let code = String(format: "%06u", value % 1_000_000)
        let digest = SHA256.hash(data: Data(code.utf8)).map { String(format: "%02x", $0) }.joined()
        return "filled code-length=6 code-sha=\(digest.prefix(12))"
    }

    private func launchProvider(_ arguments: [String], suggestingCodes: Bool = true) {
        let codes = suggestingCodes ? ["-pawSimOneTimeCodes", "1"] : []
        app.launchArguments = ["-pawAppearance", "light"] + keyArguments() + codes + arguments
        app.launch()
    }

    private func row(_ path: String) -> String {
        "autofill-entry-\(path)"
    }

    private func search(_ text: String) {
        let search = app.searchFields.firstMatch
        XCTAssertTrue(waitToExist(search, timeout: Self.patience), "The list should have a search field")
        focus(search)
        search.typeText(text)
    }

    /// The fixture's GitHub code was filled: one of the last few codes, as
    /// the test may have taken a while to read it.
    private func expectGitHubCode(file: StaticString = #filePath, line: UInt = #line) {
        expectLabel("autofill-result", contains: "filled code-length=6", file: file, line: line)
        let got = element("autofill-result").label
        let now = Date()
        let recent = (-3...1).map { Self.codeFill(at: now.addingTimeInterval(Double($0) * 30)) }
        XCTAssertTrue(recent.contains(got), "The code should be GitHub's current one: \(got)", file: file, line: line)
    }

    private func fillWithKey() {
        enterPin(Self.pin)
        touchTheKey()
    }

    func testTheCodeListShowsAnEntryOnceItIsKnownToHaveACode() {
        launchProvider(["-pawHostProvider", "code:https://github.com"])
        expectLabel("autofill-no-match", contains: "No codes for this site.")
        expectAbsent(row("web/github.com"), "Not known to have a code yet")
        search("github")
        expectLabel("autofill-site-section", contains: "For github.com")
        tap(row("web/github.com"))
        expectLabel("key-sheet-purpose", contains: "Fill code for github.com")
        fillWithKey()
        expectGitHubCode()
        expectValue("harness-codes", "web/github.com")
        waitForKeyTasksToEnd()

        // Now listed, and filled from the Face ID copy the key left.
        tap("harness-again")
        expectLabel("autofill-site-section", contains: "For github.com")
        expectLabel(row("web/github.com"), contains: "opens with Face ID")
        expectAbsent(row("web/gitlab.com"), "Another site's entry should not be offered")
        snapshot("light-code-list")
        tap(row("web/github.com"))
        expectGitHubCode()
    }

    func testAPasswordFillLearnsTheCode() {
        launchProvider(["-pawHostProvider", "https://github.com,code:https://github.com"])
        tap(row("web/github.com"))
        fillWithKey()
        expectLabel("autofill-result", contains: "filled user=you@example.com")
        expectValue("harness-codes", "web/github.com")
        waitForKeyTasksToEnd()

        tap("harness-again")
        tap(row("web/github.com"))
        expectGitHubCode()
    }

    func testAnEntryWithoutACodeSaysSo() {
        launchProvider(["-pawHostProvider", "code:https://gitlab.com"])
        search("gitlab")
        tap(row("web/gitlab.com"))
        fillWithKey()
        let problem = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS %@", "gitlab.com has no one-time code")).firstMatch
        XCTAssertTrue(waitToExist(problem, timeout: Self.patience), "It should say the entry has no code")
        XCTAssertFalse(element("autofill-result").exists, "Nothing should be filled")
        let back = app.buttons["Back to the list"]
        XCTAssertTrue(waitToExist(back, timeout: Self.patience), "It should offer the list again")
        back.tap()
        expect(row("web/gitlab.com"))
    }

    func testAnotherSitesCodeAsksFirst() {
        launchProvider(["-pawHostProvider", "code:https://paypa1.example"])
        search("github")
        tap(row("web/github.com"))
        expectLabel("autofill-confirm-question", contains: "Fill github.com's code here?")
        expectLabel("autofill-confirm-question", contains: "This code isn't saved for this site.")
        expectAbsent("pin-field", "Nothing should be opened before the confirm")
        tapUntilGone("autofill-fill-anyway")
        expectLabel("key-sheet-purpose", contains: "Fill code for github.com on paypa1.example")
        fillWithKey()
        expectGitHubCode()
    }

    func testALookalikeSiteIsNotOfferedACode() {
        launchProvider(["-pawHostProvider", "https://github.com,code:https://github.com.attacker.example"])
        tap(row("web/github.com"))
        fillWithKey()
        expectValue("harness-codes", "web/github.com")
        waitForKeyTasksToEnd()

        tap("harness-again")
        expectLabel("autofill-no-match", contains: "github.com.attacker.example")
        expectAbsent(row("web/github.com"), "A lookalike must not be offered github.com's code")
    }

    func testWithSuggestionsOffNothingIsKeptAndSearchStillFillsACode() {
        launchProvider(["-pawHostProvider", "code:https://github.com"], suggestingCodes: false)
        expectLabel("autofill-no-match", contains: "No codes for this site. Search for one.")
        search("github")
        tap(row("web/github.com"))
        fillWithKey()
        expectGitHubCode()
        expectValue("harness-codes", "none")
        waitForKeyTasksToEnd()

        tap("harness-again")
        expectLabel("autofill-no-match", contains: "No codes for this site.")
        expectAbsent(row("web/github.com"), "Nothing should be kept while suggestions are off")
    }

    func testACodeSuggestionFillsTheCode() {
        launchProvider(["-pawHostProviderEntry", "code:web/github.com"])
        expectValue("autofill-quicktype", "userInteractionRequired")
        expectLabel("key-sheet-purpose", contains: "Fill code for github.com")
        fillWithKey()
        expectGitHubCode()
    }
}
