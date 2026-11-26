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
        expectLabel("autofill-no-match", contains: "github.com.attacker.example")
        expectAbsent(row("web/github.com"), "A lookalike site must not be offered github.com")
    }

    private func search(_ text: String) {
        let search = app.searchFields.firstMatch
        XCTAssertTrue(waitToExist(search, timeout: Self.patience), "The list should have a search field")
        focus(search)
        search.typeText(text)
    }

    func testSearchFindsAnyEntry() {
        launchProvider(["-pawHostProvider", "https://nothing.example"])
        expect("autofill-no-match")
        search("fastm")
        expect(row("email/fastmail"))
        expectLabel("autofill-other-section", contains: "Other passwords")
    }

    // MARK: Another site's password

    func testAnotherSitesPasswordAsksFirstAndCancelFillsNothing() {
        launchProvider(["-pawHostProvider", "https://paypa1.example"])
        search("fastm")
        tap(row("email/fastmail"))
        expectLabel("autofill-confirm-question", contains: "Fill fastmail's password here?")
        expectLabel("autofill-confirm-question", contains: "This password isn't saved for this site.")
        expectAbsent("pin-field", "Nothing should be opened before the confirm")
        snapshot("light-confirm")
        tap("autofill-confirm-cancel")
        expect(row("email/fastmail"))
        waitForKeyTasksToEnd()
        tap("autofill-cancel")
        expectResult("ended userCanceled")
    }

    func testFillAnywayFillsWithTheKeyNamingTheSite() {
        launchProvider(["-pawHostProvider", "https://paypa1.example"])
        search("fastm")
        tap(row("email/fastmail"))
        tapUntilGone("autofill-fill-anyway")
        expectLabel("key-sheet-purpose", contains: "Fill fastmail on paypa1.example")
        enterPin(Self.pin)
        touchTheKey()
        expectResult("filled user=you@example.com")
    }

    func testTheSitesOwnPasswordFoundBySearchDoesNotAsk() {
        launchProvider(["-pawHostProvider", "https://github.com"])
        search("github")
        expectLabel("autofill-site-section", contains: "For github.com")
        tap(row("web/github.com"))
        expectLabel("key-sheet-purpose", contains: "Fill github.com")
        expectAbsent("autofill-fill-anyway", "The site's own password should not ask first")
    }

    func testAnAppWithNoSiteAsksFirst() {
        launchProvider(["-pawHostProvider", "app"])
        expectLabel("autofill-no-match", contains: "paw can't tell which site this is.")
        search("fastm")
        tap(row("email/fastmail"))
        expectLabel("autofill-confirm-question", contains: "Fill fastmail's password?")
        tapUntilGone("autofill-fill-anyway")
        expectLabel("key-sheet-purpose", contains: "Fill fastmail")
    }

    // MARK: Entries named without a domain

    /// The harness's `finance/northwind`, named like www.northwind.example.
    private static let northwind = "finance/northwind"

    /// From the list: northwind, confirmed, with PIN and touch.
    private func useNorthwindHere() {
        tap(row(Self.northwind))
        tap("autofill-use-here")
        expectLabel("key-sheet-purpose", contains: "Fill northwind on www.northwind.example")
        enterPin(Self.pin)
        touchTheKey()
        expectResult("filled user=you@example.com")
        expectValue("harness-pairings", "\(Self.northwind)=northwind.example")
        waitForKeyTasksToEnd()
    }

    func testANameMatchIsListedApartAndAsksFirst() {
        launchProvider(["-pawHostProvider", "https://www.northwind.example/login"])
        expectLabel("autofill-maybe-section", contains: "Might be for this site")
        expect(row(Self.northwind))
        expectAbsent("autofill-site-section", "A name match is not one of the site's own")
        snapshot("light-maybe-list")
        tap(row(Self.northwind))
        expectLabel("autofill-confirm-question", contains: "Use northwind on northwind.example?")
        expectLabel("autofill-confirm-question", contains: "First time on this site. Check the address is right.")
        expectAbsent("warning-row", "The first time is a question, not a warning")
        expectLabel("autofill-confirm-address", contains: "www.northwind.example")
        expectLabel("autofill-confirm-cancel", contains: "Cancel")
        expect("autofill-use-here")
        expectAbsent("pin-field", "Nothing should be opened before the confirm")
        snapshot("light-first-time")
    }

    func testCancelOnAFirstTimeMatchFillsAndRecordsNothing() {
        launchProvider(["-pawHostProvider", "https://www.northwind.example"])
        tap(row(Self.northwind))
        tap("autofill-confirm-cancel")
        expect(row(Self.northwind))
        waitForKeyTasksToEnd()
        tap("autofill-cancel")
        expectResult("ended userCanceled")
        expectValue("harness-pairings", "none")
        // Still a stranger: it asks again.
        tap("harness-again")
        expectLabel("autofill-maybe-section", contains: "Might be for this site")
        tap(row(Self.northwind))
        expect("autofill-use-here")
    }

    func testUseItHereFillsThenTheSiteFillsWithoutAsking() {
        launchProvider(["-pawHostProvider", "https://www.northwind.example"])
        useNorthwindHere()

        tap("harness-again")
        expectLabel("autofill-site-section", contains: "For www.northwind.example")
        expectAbsent("autofill-maybe-section", "A paired name match is the site's own")
        expectLabel(row(Self.northwind), contains: "opens with Face ID")
        tap(row(Self.northwind))
        expectResult("filled user=you@example.com")
        expectAbsent("autofill-use-here", "A paired name match should not ask again")
    }

    func testALookalikeOfAPairedSiteIsNotOfferedAndWarns() {
        launchProvider(["-pawHostProvider", "https://www.northwind.example,https://login.northwind.test"])
        useNorthwindHere()

        tap("harness-again")
        expectLabel("autofill-no-match", contains: "login.northwind.test")
        expectAbsent(row(Self.northwind), "A lookalike must not be offered northwind")
        search("northwind")
        expectLabel("autofill-other-section", contains: "Other passwords")
        tap(row(Self.northwind))
        expectLabel("autofill-confirm-question", contains: "Use northwind on northwind.test?")
        expectLabel("autofill-confirm-question", contains: "Used before on northwind.example.")
        expectLabel("autofill-confirm-address", contains: "login.northwind.test")
        expectAbsent("pin-field", "Nothing should be opened before the confirm")
        snapshot("light-lookalike")
        tap("autofill-confirm-cancel")
        expect(row(Self.northwind))

        // Used here anyway: it fills this once (Face ID, from the copy the
        // first fill left) and is never paired.
        tap(row(Self.northwind))
        tap("autofill-use-here")
        expectResult("filled user=you@example.com")
        expectValue("harness-pairings", "\(Self.northwind)=northwind.example")
        waitForKeyTasksToEnd()

        // So the next time still warns.
        tap("harness-again")
        expectLabel("autofill-no-match", contains: "login.northwind.test")
        search("northwind")
        tap(row(Self.northwind))
        expectLabel("autofill-confirm-question", contains: "Used before on northwind.example.")
    }

    func testAPasswordFromSearchForAnotherSiteIsNeverPaired() {
        launchProvider(["-pawHostProvider", "https://www.northwind.example"])
        search("fastm")
        tap(row("email/fastmail"))
        tapUntilGone("autofill-fill-anyway")
        enterPin(Self.pin)
        touchTheKey()
        expectResult("filled user=you@example.com")
        expectValue("harness-pairings", "none")
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
        expectLabel("warning-row", contains: "Face ID was cancelled")
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

    func testBusyStoreTriesAgain() {
        launchProvider(["-pawHostProvider", "https://github.com", "-pawHostProviderStore", "busy"])
        expectLabel("warning-row", contains: "The store is busy")
        tap("autofill-retry")
        expect(row("web/github.com"))
    }

    func testAnotherKeysEntrySaysSoBeforeThePin() {
        launchProvider(["-pawHostProvider", "https://nothing.example"])
        search("old-forum")
        tap(row("archive/old-forum"))
        tapUntilGone("autofill-fill-anyway")
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
        expectResult("ended userCanceled")
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
        expectResult("ended userCanceled")
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
        expectValue("autofill-quicktype", "userInteractionRequired")
        expectResult("ended credentialIdentityNotFound")
    }
}
