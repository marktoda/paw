import XCTest

/// Drives the real app in the simulator against a simulated key, the way a
/// person would: tap, type, touch the key, read what the screen says.
final class KeyTests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
        app = XCUIApplication()
    }

    override func tearDown() {
        // Every test leaves a picture of where it ended, pass or fail.
        snapshot("end")
    }

    private func launch(touch: Bool = true, touchTimeoutMs: Int = 15_000) {
        app.launchArguments = [
            "-pawSimPin", "123456",
            "-pawSimTouch", touch ? "1" : "0",
            "-pawSimTouchTimeoutMs", String(touchTimeoutMs),
        ]
        app.launch()
    }

    private func snapshot(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = "\(self.name.split(separator: " ").last?.dropLast() ?? "test")-\(name)"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func enterPin(_ pin: String) {
        let field = app.secureTextFields["pin-field"]
        XCTAssertTrue(field.waitForExistence(timeout: 10), "PIN field should be on screen")
        field.tap()
        if let existing = field.value as? String, !existing.isEmpty, existing != "Key PIN" {
            field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: existing.count))
        }
        field.typeText(pin)
    }

    private func tapDecrypt() {
        let button = app.buttons["decrypt"]
        XCTAssertTrue(button.isEnabled, "Decrypt should be enabled once a PIN is typed")
        button.tap()
    }

    /// Waits until the newest result contains `text`, and fails with what the
    /// screen actually said otherwise.
    private func expectResult(containing text: String, timeout: TimeInterval = 15,
                              file: StaticString = #filePath, line: UInt = #line) {
        let latest = app.staticTexts["latest-result"]
        let found = NSPredicate(format: "label CONTAINS %@", text)
        let outcome = XCTWaiter().wait(
            for: [XCTNSPredicateExpectation(predicate: found, object: latest)], timeout: timeout)
        if outcome != .completed {
            let shown = latest.exists ? latest.label : "(no result on screen)"
            XCTFail("Expected a result containing \"\(text)\", screen showed: \(shown)",
                    file: file, line: line)
        }
    }

    func testReadKeyShowsIdentityWithoutAPin() {
        launch()
        app.buttons["read-key"].tap()

        expectResult(containing: "Key: 0006:12345678")
        expectResult(containing: "PIN tries left: 3")
        expectResult(containing: "Touch to decrypt: every time")
    }

    func testDecryptWaitsForTouchThenSucceeds() {
        launch()
        enterPin("123456")
        tapDecrypt()

        XCTAssertTrue(app.staticTexts["touch-prompt"].waitForExistence(timeout: 10),
                      "The touch prompt should appear once the PIN is accepted")
        XCTAssertFalse(app.staticTexts["latest-result"].exists,
                       "Nothing should be decrypted before the key is touched")
        snapshot("waiting-for-touch")

        app.buttons["simulated-touch"].tap()

        expectResult(containing: "Decrypted correctly.")
        XCTAssertFalse(app.staticTexts["touch-prompt"].exists,
                       "The touch prompt should go away after the decrypt")
    }

    func testCardChecksReportEveryCommand() {
        launch()
        enterPin("123456")
        app.buttons["card-checks"].tap()

        // Three signatures each wait for their own touch.
        let latest = app.staticTexts["latest-result"]
        let deadline = Date().addingTimeInterval(30)
        while !latest.exists && Date() < deadline {
            let touch = app.buttons["simulated-touch"]
            if touch.waitForExistence(timeout: 1) { touch.tap() }
        }

        expectResult(containing: "Sign a login, chained, last part (touch)")
        XCTAssertFalse(latest.label.contains("error:"), latest.label)
        XCTAssertFalse(latest.label.contains("123456"), "Results must never show the PIN")
        snapshot("card-checks")
    }

    func testTouchTimeoutCheckReportsAnUntouchedKey() {
        launch(touchTimeoutMs: 1_000)
        enterPin("123456")
        app.buttons["touch-timeout-check"].tap()

        expectResult(containing: "6982  ")
        expectResult(containing: "Sign a login with no touch")
    }

    func testDecryptWithoutTouchPolicyNeedsNoTouch() {
        launch(touch: false)
        enterPin("123456")
        tapDecrypt()

        expectResult(containing: "Decrypted correctly.")
        XCTAssertFalse(app.staticTexts["touch-prompt"].exists)
    }

    func testUntouchedKeyFailsInsteadOfHanging() {
        launch(touchTimeoutMs: 1_500)
        enterPin("123456")
        tapDecrypt()

        expectResult(containing: "The key did not decrypt this")
        XCTAssertFalse(app.staticTexts["touch-prompt"].exists)
        XCTAssertTrue(app.buttons["decrypt"].isEnabled, "The screen should be usable again")
    }

    func testWrongPinCountsDownAndThenLocks() {
        launch()
        enterPin("000000")

        tapDecrypt()
        expectResult(containing: "Wrong PIN. 2 tries left.")
        XCTAssertFalse(app.staticTexts["touch-prompt"].exists,
                       "A wrong PIN must never reach the touch step")

        tapDecrypt()
        expectResult(containing: "Wrong PIN. 1 try left before the key locks.")
        snapshot("one-try-left")

        tapDecrypt()
        expectResult(containing: "The PIN is locked.")

        app.buttons["read-key"].tap()
        expectResult(containing: "PIN tries left: 0")
    }

    func testCorrectPinAfterAMistakeRestoresTheTries() {
        launch(touch: false)
        enterPin("000000")
        tapDecrypt()
        expectResult(containing: "Wrong PIN. 2 tries left.")

        enterPin("123456")
        tapDecrypt()
        expectResult(containing: "Decrypted correctly.")

        app.buttons["read-key"].tap()
        expectResult(containing: "PIN tries left: 3")
    }
}
