import XCTest

/// AutoFill's checks beyond an entry's name, in the debug harness
/// (`-pawHostProvider`): an entry whose own `url:` line names another
/// site, and international domain names. The rules themselves are tested
/// in the core (core/tests/site_checks.rs).
final class AutoFillRestTests: PawUITestCase {
    /// Holds `url: https://tailspin.test`.
    private static let tailspin = "shop/tailspin.example"
    /// `web/bücher.example`, holding `url: https://bücher.example`.
    private static let bucher = "web/b\u{00FC}cher.example"

    private func launchProvider(_ arguments: [String]) {
        app.launchArguments = ["-pawAppearance", "light"] + keyArguments() + arguments
        app.launch()
    }

    private func row(_ path: String) -> String {
        "autofill-entry-\(path)"
    }

    /// The site's own entry by host, opened with the key, then the warning.
    private func openTailspin() {
        launchProvider(["-pawHostProvider", "https://www.tailspin.example/login"])
        expectLabel("autofill-site-section", contains: "For www.tailspin.example")
        tap(row(Self.tailspin))
        enterPin(Self.pin)
        touchTheKey()
        expectLabel("autofill-confirm-question", contains: "Saved for tailspin.test, not tailspin.example")
        expectLabel("autofill-confirm-address", contains: "www.tailspin.example")
        expectLabel("autofill-confirm-cancel", contains: "Cancel")
    }

    func testAUrlForAnotherSiteWarnsBeforeFillingAndCancelFillsNothing() {
        openTailspin()
        snapshot("light-url-differs")
        tap("autofill-confirm-cancel")
        expect(row(Self.tailspin))
        waitForKeyTasksToEnd()
        tap("autofill-cancel")
        expectLabel("autofill-result", contains: "ended userCanceled")
    }

    func testFillAnywayAfterTheUrlWarningFillsAndPairsNothing() {
        openTailspin()
        // Opened again from the Face ID copy the key just kept.
        tap("autofill-fill-anyway")
        expectLabel("autofill-result", contains: "filled user=you@example.com")
        expectValue("harness-pairings", "none")
    }

    func testAnInternationalNameMatchesItsAsciiFormAndShowsInAscii() {
        launchProvider(["-pawHostProvider", "https://xn--bcher-kva.example/login"])
        expectLabel("autofill-site-section", contains: "For xn--bcher-kva.example")
        tap(row(Self.bucher))
        expectLabel("key-sheet-purpose", contains: "Fill b\u{00FC}cher.example")
        enterPin(Self.pin)
        touchTheKey()
        // Its url line is the same site in Unicode: no warning.
        expectLabel("autofill-result", contains: "filled user=you@example.com")
    }

    func testACyrillicLookalikeShowsInAsciiAndIsNotOffered() {
        // `fаbrikam.example` with a Cyrillic а.
        launchProvider(["-pawHostProvider", "https://xn--fbrikam-2fg.example"])
        expectLabel("autofill-no-match", contains: "xn--fbrikam-2fg.example")
        expectAbsent(row("web/fabrikam.example"), "A lookalike must not be offered fabrikam.example")
    }
}
