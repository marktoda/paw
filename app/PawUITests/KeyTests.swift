import XCTest

/// Settings › Key diagnostics with the simulated key: what the key said
/// when it was plugged in, the touch timeout check (a key task, as in the
/// device build), and which build this is.
final class KeyTests: PawUITestCase {
    private func openDiagnostics() {
        openSettings()
        tap("settings-key-diagnostics")
        expect("diagnostics-done")
    }

    /// A line of the key log that contains `text`, scrolled to: the list
    /// builds its rows lazily.
    private func keyLogLine(containing text: String) -> XCUIElement {
        let found = app.staticTexts.matching(
            NSPredicate(format: "identifier == 'key-log-line' AND label CONTAINS %@", text)).firstMatch
        var swipes = 0
        while !waitToExist(found, timeout: 2) && swipes < 6 {
            app.swipeUp()
            swipes += 1
        }
        return found
    }

    func testDiagnosticsShowWhatTheKeySaidWithoutAPin() {
        launchFixture()
        openDiagnostics()

        expectLabel("diagnostics-serial", contains: "12 345 678")
        expectLabel("diagnostics-tries", contains: "3")
        expectLabel("diagnostics-touch-open", contains: "Every time")
        expectLabel("diagnostics-touch-ssh", contains: "Every time")
        expectAbsent("pin-field", "Reading what the key said needs no PIN")
    }

    func testTheTouchTimeoutCheckReportsAnUntouchedKeyAndLogsWhatItAnswered() {
        // Longer than a second, or the key's refusal is too quick to be
        // called a timeout; short enough not to keep the test waiting.
        launchFixture(touchTimeoutMs: 4_000)
        openDiagnostics()

        tap("touch-timeout-check")
        enterPin(Self.pin)
        expect("touch-timeout-waiting")
        expectLabel("touch-timeout-result", contains: "gave up waiting for a touch", timeout: 20)
        snapshot("touch-timeout")
        XCTAssertTrue(keyLogLine(containing: "task touch timeout check").exists,
                      "The key log should name the check")
        XCTAssertTrue(keyLogLine(containing: "PIN check 9000: a touch timeout").exists,
                      "The key log should have what the key answered after the timeout")

        // The PIN is held: a second check asks for none.
        tap("touch-timeout-check")
        expect("touch-timeout-waiting")
        expectAbsent("pin-field", "The PIN should still be held after a touch timeout")
        expectLabel("touch-timeout-result", contains: "gave up waiting for a touch", timeout: 20)
    }

    func testSettingsSayWhichBuildThisIs() {
        launchFixture()
        openSettings()
        scrollTo("settings-build")
        expectLabel("settings-build", contains: "Debug")
    }
}
