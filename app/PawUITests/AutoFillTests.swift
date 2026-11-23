import CryptoKit
import XCTest

/// The AutoFill provider's screens, hosted in the app by the debug harness
/// (`-pawHostProvider`): the simulator cannot run a real AutoFill session
/// from another app. The harness shows what a fill hands over as the user
/// name, the password's length and the start of its SHA-256, never the
/// password. The real extension process, the system Face ID prompt and
/// the bar above the keyboard are checked by hand on a device.
final class AutoFillTests: PawUITestCase {
    /// What the harness reports for a fill of the fixture's web/github.com.
    private static let githubFill: String = {
        let digest = SHA256.hash(data: Data(githubPassword.utf8)).map { String(format: "%02x", $0) }.joined()
        return "filled user=you@example.com password-length=\(githubPassword.count) password-sha=\(digest.prefix(12))"
    }()

    private func launchProvider(_ arguments: [String], appearance: String = "light") {
        app.launchArguments = ["-pawAppearance", appearance] + keyArguments() + arguments
        app.launch()
    }

    private func expectResult(_ text: String, file: StaticString = #filePath, line: UInt = #line) {
        expectLabel("autofill-result", contains: text, file: file, line: line)
    }

    private func row(_ path: String) -> String {
        "autofill-entry-\(path)"
    }

    /// From the list: the entry with PIN and touch, then the harness's result.
    private func fillGitHubWithKey(appearance: String? = nil) {
        tap(row("web/github.com"))
        expectLabel("key-sheet-purpose", contains: "Fill github.com")
        if let appearance {
            pinField()
            snapshot("\(appearance)-key-sheet")
        }
        enterPin(Self.pin)
        touchTheKey()
        expectResult(Self.githubFill)
    }

    // MARK: The list

    func testListOffersTheSiteFirst() {
        launchProvider(["-pawHostProvider", "https://github.com/login"])
        expectLabel("autofill-site-section", contains: "For github.com")
        expect(row("web/github.com"))
        expectLabel(row("web/github.com"), contains: "needs your key")
        expectAbsent(row("web/gitlab.com"), "Another site's entry should not be offered")
        snapshot("light-list")
    }

    func testExactHostComesBeforeTheSameDomain() {
        launchProvider(["-pawHostProvider", "https://example.com"])
        let exact = expect(row("web/example.com"))
        let sameDomain = expect(row("work/wiki.example.com"))
        XCTAssertLessThan(exact.frame.minY, sameDomain.frame.minY, "The exact host should be listed first")
        expectAbsent(row("web/accounts.example.org"), "example.org is another site")
    }

    func testLookalikeSiteIsNotOffered() {
        launchProvider(["-pawHostProvider", "https://github.com.attacker.example/login"])
        expectLabel("autofill-no-match", contains: "Nothing here matches github.com.attacker.example")
        expectAbsent(row("web/github.com"), "A lookalike site must not be offered github.com")
    }

    func testSearchFindsAnyEntry() {
        launchProvider(["-pawHostProvider", "https://nothing.example"])
        expect("autofill-no-match")
        let search = app.searchFields.firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: Self.patience), "The list should have a search field")
        search.tap()
        search.typeText("fastm")
        expect(row("email/fastmail"))
    }

    func testMatchingTable() {
        app.launchArguments = ["-pawMatchTable", "1"]
        app.launch()
        let summary = expect("match-table-summary")
        wait(for: "value CONTAINS 'passed' OR value CONTAINS 'failed'", on: summary, timeout: Self.patience,
             "The table should finish")
        XCTAssertTrue((summary.value as? String)?.hasPrefix("all ") == true,
                      "Every matching case should pass: \(summary.value ?? "")")
    }

    // MARK: Filling

    func testKeyFillLeavesAFaceIdCopy() {
        launchProvider(["-pawHostProvider", "https://github.com"])
        fillGitHubWithKey(appearance: "light")
        waitForKeyTasksToEnd()

        tap("harness-again")
        expectLabel(row("web/github.com"), contains: "opens with Face ID")
        tap(row("web/github.com"))
        expectResult(Self.githubFill)
    }

    func testFaceIdCancelledOffersTheKey() {
        launchProvider(["-pawHostProvider", "https://github.com", "-pawSimFaceId", "cancel"])
        fillGitHubWithKey()
        tap("harness-again")
        expectLabel(row("web/github.com"), contains: "opens with Face ID")
        tap(row("web/github.com"))
        expectLabel("warning-row", contains: "Face ID was cancelled.")
        tap("error-action")
        expectLabel("key-sheet-purpose", contains: "Fill github.com")
        enterPin(Self.pin)
        touchTheKey()
        expectResult(Self.githubFill)
    }

    func testCancelEndsTheRequest() {
        launchProvider(["-pawHostProvider", "https://github.com"])
        expect(row("web/github.com"))
        tap("autofill-cancel")
        expectResult("ended userCanceled")

        tap("harness-again")
        tap(row("web/github.com"))
        pinField()
        tap("key-sheet-cancel")
        expect(row("web/github.com"))
        waitForKeyTasksToEnd()
        tap("autofill-cancel")
        expectResult("ended userCanceled")
    }

    func testNoStoreSaysToSetUp() {
        launchProvider(["-pawHostProvider", "https://github.com", "-pawHostProviderStore", "none"])
        expect("autofill-close")
        XCTAssertTrue(app.staticTexts["Set up paw first"].exists, "It should say to set up paw")
        snapshot("light-no-store")
        tap("autofill-close")
        expectResult("ended userCanceled")
    }

    func testAnotherKeysEntrySaysSoBeforeThePin() {
        launchProvider(["-pawHostProvider", "https://nothing.example"])
        let search = app.searchFields.firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: Self.patience), "The list should have a search field")
        search.tap()
        search.typeText("old-forum")
        tap(row("archive/old-forum"))
        expectLabel("key-sheet-headline", contains: "This key can't open old-forum")
        expectAbsent("pin-field", "No PIN should be asked for an entry this key can't open")
        tap("key-sheet-cancel")
        expect(row("archive/old-forum"))
    }

    func testLockedPinSaysSo() {
        launchProvider(["-pawHostProvider", "https://github.com", "-pawSimPinTries", "1"])
        tap(row("web/github.com"))
        enterPin("999999")
        expectLabel("key-sheet-headline", contains: "Your key is locked")
        expectLabel("key-sheet-cancel", contains: "Close")
        tap("key-sheet-cancel")
        expect(row("web/github.com"))
    }

    // MARK: Going away

    func testSwipeAwayMidPinCancelsTheTask() {
        launchProvider(["-pawHostProvider", "https://github.com", "-pawHarnessDismiss", "1"])
        tap(row("web/github.com"))
        enterPin(Self.pin, send: false)
        tapSimulated("harness-dismiss")
        expectResult("dismissed")
        waitForKeyTasksToEnd()
        expectValue("harness-pin", "none")
    }

    func testSwipeAwayMidTouchCancelsTheTaskAndForgetsThePin() {
        launchProvider(["-pawHostProvider", "https://github.com", "-pawHarnessDismiss", "1"])
        tap(row("web/github.com"))
        enterPin(Self.pin)
        expect("touch-prompt")
        expectValue("harness-pin", "held")
        tapSimulated("harness-dismiss")
        expectResult("dismissed")
        // A touch after the sheet went away fills nothing.
        tap("harness-touch-key")
        waitForKeyTasksToEnd()
        expectValue("harness-pin", "none")
        XCTAssertFalse(element("autofill-result").label.contains("filled"), "Nothing should be filled")
    }

    // MARK: Suggestions above the keyboard

    func testSuggestionWithoutThenWithACopy() {
        launchProvider(["-pawHostProviderEntry", "web/github.com"])
        expectValue("autofill-quicktype", "userInteractionRequired")
        // No list: straight to the key.
        expectLabel("key-sheet-purpose", contains: "Fill github.com")
        enterPin(Self.pin)
        touchTheKey()
        expectResult(Self.githubFill)
        waitForKeyTasksToEnd()

        // Now a copy exists: Face ID, and no key sheet.
        tap("harness-again")
        expectValue("autofill-quicktype", "userInteractionRequired")
        expectResult(Self.githubFill)
    }

    func testSuggestionCancelEndsTheRequest() {
        launchProvider(["-pawHostProviderEntry", "web/github.com"])
        pinField()
        tap("key-sheet-cancel")
        expectResult("ended userCanceled")
        waitForKeyTasksToEnd()
    }

    func testSuggestionForAMissingEntry() {
        launchProvider(["-pawHostProviderEntry", "web/gone.example"])
        expectValue("autofill-quicktype", "credentialIdentityNotFound")
        expectResult("ended credentialIdentityNotFound")
    }

    // MARK: Pictures and Settings

    func testDarkListAndKeySheet() {
        launchProvider(["-pawHostProvider", "https://github.com"], appearance: "dark")
        expect(row("web/github.com"))
        snapshot("dark-list")
        fillGitHubWithKey(appearance: "dark")
    }

    func testSettingsShowsAutoFill() {
        launchFixture()
        openSettings()
        scrollTo("settings-autofill")
        snapshot("settings")
    }
}
