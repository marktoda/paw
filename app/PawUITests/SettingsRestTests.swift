import XCTest

/// Settings beyond Lock now and Remove: who can open the store, Face ID on
/// and off, sites used, the server's keys and Copy diagnostics. All on the
/// fixture store with the simulated key; the rules behind them are tested
/// in the core (core/tests/settings.rs).
final class SettingsRestTests: PawUITestCase {
    func testWhoCanOpenListsEachListOfKeys() {
        launchFixture()
        openSettings()
        tap("settings-recipients")
        // The top `.gpg-id` names the simulated key, set up as this store's.
        expectLabel("recipients-this-key", contains: "This key")
        expectText("recipients-top-footer", contains: "passwords")
        // archive/ has its own `.gpg-id`, for a key paw has no copy of.
        scrollTo("recipients-archive-footer")
        expectText("recipients-archive-footer", contains: "2 passwords")
        expectLabel("recipients-key", contains: "Not on this iPhone")
        snapshot("who-can-open")
    }

    func testFaceIdOffMeansTheKeyEveryTime() {
        launchFixture()
        openGitHubWithKey()
        goBack()
        goBack()
        expect("face-id-github.com")

        openSettings()
        setFaceId(on: false)
        expectText("settings-face-id-footer", contains: "Every password needs your key")
        snapshot("face-id-off")
        closeSettings()
        // The copy ended at once, and the next open asks for the key.
        expectGone("face-id-github.com")
        tap("folder-web")
        tap("entry-github.com")
        expectLabel("key-sheet-purpose", contains: "Open github.com")
        touchTheKey()
        expectLabel("entry-title", contains: "github.com")
        goBack()
        goBack()
        expectAbsent("face-id-github.com", "With Face ID off, an open with the key keeps no copy")

        openSettings()
        setFaceId(on: true)
        expectText("settings-face-id-footer", contains: "8 hours")
    }

    func testSitesUsedRemovesOneAndClearsAll() {
        launchFixture(extra: ["-pawSimPairings", "1"])
        openSettings()
        tap("settings-sites-used")
        let sites = app.descendants(matching: .any).matching(identifier: "sites-used-site")
        XCTAssertTrue(waitUntil(timeout: Self.patience) { sites.count == 2 }, "Both seeded sites should be listed")
        snapshot("sites-used")

        let first = sites.element(boundBy: 0)
        first.swipeLeft()
        let remove = app.buttons["Remove"]
        XCTAssertTrue(waitToExist(remove, timeout: Self.patience), "Swiping should offer Remove")
        remove.tap()
        XCTAssertTrue(waitUntil(timeout: Self.patience) { sites.count == 1 }, "One site should be left")

        tap("sites-used-clear-all")
        tap("sites-used-clear-confirm")
        expect("sites-used-empty")
    }

    func testServerKeyCanBeForgottenAndTrustedAgain() {
        launchFixture()
        openSettings()
        // The simulated host is not one whose keys ship with paw.
        expectText("settings-host-fingerprint", contains: "Trusted by you")
        tap("settings-host-fingerprint")
        expect("settings-host-fingerprints")
        expectLabel("settings-host-key", contains: "Ed25519")
        expectLabel("settings-host-key", contains: "SHA256:")
        snapshot("server-keys")

        tap("settings-forget-host-key")
        tap("settings-forget-host-confirm")
        expectText("settings-host-fingerprint", contains: "Not trusted yet")
        tap("settings-check-host")
        expectLabel("settings-presented-key", contains: "SHA256:")
        tap("settings-trust-host")
        expectText("settings-host-fingerprint", contains: "Trusted by you")
    }

    func testCopyDiagnosticsNamesNothing() {
        launchFixture()
        openGitHubWithKey()
        goBack()
        goBack()
        openSettings()
        tap("settings-key-diagnostics")
        tap("diagnostics-copy")
        expectLabel("diagnostics-copy", contains: "Diagnostics copied")
        let report = expect("diagnostics-report")
        expectLabel("diagnostics-report", contains: "Store: set up")
        expectLabel("diagnostics-report", contains: "Key: plugged in")
        for name in ["github.com", "web/", "archive", "127.0.0.1"] {
            XCTAssertFalse(report.label.contains(name), "\"\(name)\" is in the diagnostics")
        }
        snapshot("diagnostics")
    }

    // MARK: Helpers

    /// Flips Settings' Face ID switch until it reads `on`.
    private func setFaceId(on: Bool) {
        let toggle = scrollTo("settings-face-id")
        let want = on ? "1" : "0"
        for _ in 0..<3 where (toggle.value as? String) != want {
            // iOS draws the switch inside the row: tap the switch itself.
            let inner = toggle.switches.firstMatch
            (inner.exists ? inner : toggle).tap()
            if waitUntil(timeout: 2, { (toggle.value as? String) == want }) { break }
        }
        XCTAssertEqual(toggle.value as? String, want, "Face ID should be \(on ? "on" : "off")")
    }
}
