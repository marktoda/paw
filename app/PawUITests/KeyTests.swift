import XCTest

/// Drives the real app in the simulator against a simulated key, the way a
/// person would: tap, type, touch the key, read what the screen says.
final class KeyTests: PawUITestCase {
    private func launch(touch: Bool = true, touchTimeoutMs: Int = PawUITestCase.touchTimeoutMs) {
        app.launchArguments = [
            "-pawSelfTest", "1",
            "-pawSimPin", Self.pin,
            "-pawSimTouch", touch ? "1" : "0",
            "-pawSimTouchTimeoutMs", String(touchTimeoutMs),
        ]
        app.launch()
    }

    /// Types the PIN into the test screen's field, which has no return
    /// action: the buttons below use it.
    private func typePin(_ pin: String, file: StaticString = #filePath, line: UInt = #line) {
        enterPin(pin, send: false, file: file, line: line)
    }

    private func tapDecrypt(file: StaticString = #filePath, line: UInt = #line) {
        expectEnabled("decrypt", true, file: file, line: line)
        tapWhenHittable(app.buttons["decrypt"], "Decrypt", file: file, line: line)
    }

    /// Waits until the newest result contains `text`, and fails with what the
    /// screen actually said otherwise.
    private func expectResult(containing text: String, timeout: TimeInterval = PawUITestCase.patience,
                              file: StaticString = #filePath, line: UInt = #line) {
        let latest = app.staticTexts["latest-result"]
        let found = NSPredicate(format: "label CONTAINS %@", text)
        if !waitUntil(timeout: timeout, { latest.exists && found.evaluate(with: latest) }) {
            let shown = latest.exists ? latest.label : "(no result on screen)"
            XCTFail("Expected a result containing \"\(text)\", screen showed: \(shown)",
                    file: file, line: line)
        }
    }

    func testReadKeyShowsIdentityWithoutAPin() {
        launch()
        tapWhenHittable(app.buttons["read-key"], "Read key")

        expectResult(containing: "Key: 0006:12345678")
        expectResult(containing: "PIN tries left: 3")
        expectResult(containing: "Touch to decrypt: every time")
    }

    func testDecryptWaitsForTouchThenSucceeds() {
        launch()
        typePin(Self.pin)
        tapDecrypt()

        XCTAssertTrue(waitToExist(app.staticTexts["touch-prompt"], timeout: Self.patience),
                      "The touch prompt should appear once the PIN is accepted")
        XCTAssertFalse(app.staticTexts["latest-result"].exists,
                       "Nothing should be decrypted before the key is touched")
        snapshot("waiting-for-touch")

        tapSimulated("simulated-touch")

        expectResult(containing: "Decrypted correctly.")
        expectGone(app.staticTexts["touch-prompt"], "The touch prompt should go away after the decrypt")
    }

    func testCardChecksReportEveryCommand() {
        launch()
        typePin(Self.pin)
        expectEnabled("card-checks", true)
        tapWhenHittable(app.buttons["card-checks"], "Card checks")

        // Three signatures each wait for their own touch: touch whenever
        // the button is there, until the result is.
        let latest = app.staticTexts["latest-result"]
        let touch = app.buttons["simulated-touch"]
        let deadline = Date().addingTimeInterval(Self.patience)
        while !latest.exists && Date() < deadline {
            if waitUntil(timeout: 1, { touch.exists && touch.isHittable }) { touch.tap() }
        }

        expectResult(containing: "Sign a login, chained, last part (touch)")
        XCTAssertFalse(latest.label.contains("error:"), latest.label)
        XCTAssertFalse(latest.label.contains("123456"), "Results must never show the PIN")
        snapshot("card-checks")
    }

    func testTouchTimeoutCheckReportsAnUntouchedKey() {
        launch(touchTimeoutMs: 1_000)
        typePin(Self.pin)
        expectEnabled("touch-timeout-check", true)
        tapWhenHittable(app.buttons["touch-timeout-check"], "The touch timeout check")

        expectResult(containing: "6982  ", timeout: 15)
        expectResult(containing: "Sign a login with no touch")
    }

    func testDecryptWithoutTouchPolicyNeedsNoTouch() {
        launch(touch: false)
        typePin(Self.pin)
        tapDecrypt()

        expectResult(containing: "Decrypted correctly.")
        XCTAssertFalse(app.staticTexts["touch-prompt"].exists)
    }

    func testUntouchedKeyFailsInsteadOfHanging() {
        launch(touchTimeoutMs: 1_500)
        typePin(Self.pin)
        tapDecrypt()

        expectResult(containing: "The key did not decrypt this", timeout: 15)
        expectGone(app.staticTexts["touch-prompt"], "The touch prompt should go away when the key gives up")
        // Each run forgets the PIN typed for it; typed again, it runs again.
        XCTAssertFalse(app.buttons["decrypt"].isEnabled, "The PIN should be cleared after a run")
        typePin(Self.pin)
        expectEnabled("decrypt", true)
    }

    func testWrongPinCountsDownAndThenLocks() {
        launch()
        typePin("000000")

        tapDecrypt()
        expectResult(containing: "Wrong PIN. 2 tries left.")
        XCTAssertFalse(app.staticTexts["touch-prompt"].exists,
                       "A wrong PIN must never reach the touch step")

        typePin("000000")
        tapDecrypt()
        expectResult(containing: "Wrong PIN. 1 try left before the key locks.")
        snapshot("one-try-left")

        typePin("000000")
        tapDecrypt()
        expectResult(containing: "The PIN is locked.")

        tapWhenHittable(app.buttons["read-key"], "Read key")
        expectResult(containing: "PIN tries left: 0")
    }

    func testCorrectPinAfterAMistakeRestoresTheTries() {
        launch(touch: false)
        typePin("000000")
        tapDecrypt()
        expectResult(containing: "Wrong PIN. 2 tries left.")

        typePin(Self.pin)
        tapDecrypt()
        expectResult(containing: "Decrypted correctly.")

        tapWhenHittable(app.buttons["read-key"], "Read key")
        expectResult(containing: "PIN tries left: 3")
    }
}
