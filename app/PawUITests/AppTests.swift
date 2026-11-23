import XCTest

/// The real screens on the fixture store (`-pawFixture 1`): 27 entries the
/// simulated key can open and 2 under `archive/` it cannot, a simulated key
/// with PIN 123456, a simulated Face ID and a simulated clock.
final class AppTests: XCTestCase {
    private var app: XCUIApplication!

    private static let pin = "123456"
    private static let githubPassword = "tidal-Quartz-88-ferns"

    override func setUp() {
        continueAfterFailure = false
        app = XCUIApplication()
    }

    override func tearDown() {
        snapshot("end")
    }

    // MARK: Helpers

    private func launch(appearance: String = "light", extra: [String] = []) {
        app.launchArguments = [
            "-pawFixture", "1",
            "-pawAppearance", appearance,
            "-pawSimPin", Self.pin,
            "-pawSimTouch", "1",
            "-pawSimTouchTimeoutMs", "15000",
        ] + extra
        app.launch()
        expect("home-title", timeout: 20)
    }

    private func snapshot(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = "\(self.name.split(separator: " ").last?.dropLast() ?? "test")-\(name)"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func element(_ identifier: String) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: identifier).firstMatch
    }

    @discardableResult
    private func expect(_ identifier: String, timeout: TimeInterval = 10,
                        file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let found = element(identifier)
        XCTAssertTrue(found.waitForExistence(timeout: timeout),
                      "\"\(identifier)\" should be on screen", file: file, line: line)
        return found
    }

    private func expectGone(_ identifier: String, timeout: TimeInterval = 10,
                            file: StaticString = #filePath, line: UInt = #line) {
        let gone = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "exists == false"), object: element(identifier))
        if XCTWaiter().wait(for: [gone], timeout: timeout) != .completed {
            XCTFail("\"\(identifier)\" should have gone", file: file, line: line)
        }
    }

    private func expectAbsent(_ identifier: String, _ why: String,
                              file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertFalse(element(identifier).exists, why, file: file, line: line)
    }

    /// Waits until the element's label contains `text`, and reports what it said otherwise.
    private func expectLabel(_ identifier: String, contains text: String, timeout: TimeInterval = 10,
                             file: StaticString = #filePath, line: UInt = #line) {
        let found = expect(identifier, timeout: timeout, file: file, line: line)
        let matches = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "label CONTAINS %@", text), object: found)
        if XCTWaiter().wait(for: [matches], timeout: timeout) != .completed {
            XCTFail("\"\(identifier)\" should say \"\(text)\", it said \"\(found.label)\"",
                    file: file, line: line)
        }
    }

    private func expectValue(_ identifier: String, _ value: String,
                             file: StaticString = #filePath, line: UInt = #line) {
        let found = expect(identifier, file: file, line: line)
        let matches = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "value == %@", value), object: found)
        if XCTWaiter().wait(for: [matches], timeout: 5) != .completed {
            XCTFail("\"\(identifier)\" should be \"\(value)\", it was \"\(found.value ?? "nil")\"",
                    file: file, line: line)
        }
    }

    /// Scrolls the list until the element can be tapped.
    @discardableResult
    private func scrollTo(_ identifier: String, file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let found = element(identifier)
        var swipes = 0
        while !(found.exists && found.isHittable) && swipes < 6 {
            app.swipeUp()
            swipes += 1
        }
        XCTAssertTrue(found.exists, "\"\(identifier)\" should be in the list", file: file, line: line)
        return found
    }

    private func tap(_ identifier: String, file: StaticString = #filePath, line: UInt = #line) {
        scrollTo(identifier, file: file, line: line).tap()
    }

    private func goBack() {
        let back = app.navigationBars.buttons["BackButton"]
        if back.exists {
            back.tap()
        } else {
            app.navigationBars.buttons.element(boundBy: 0).tap()
        }
    }

    /// The PIN field, secure or shown (it is shown with one try left).
    @discardableResult
    private func pinField(file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let field = element("pin-field")
        XCTAssertTrue(field.waitForExistence(timeout: 10), "The key sheet should ask for the PIN",
                      file: file, line: line)
        return field
    }

    private func enterPin(_ pin: String) {
        let field = pinField()
        field.tap()
        field.typeText(pin)
        app.buttons["pin-continue"].tap()
    }

    /// A simulator control drawn below the key sheet's own buttons. Tapped
    /// at once: a touch must land before the simulated key's timeout, and
    /// XCUITest scrolls the sheet to it if needed.
    private func tapSimulated(_ identifier: String) {
        app.buttons[identifier].tap()
    }

    private func touchTheKey() {
        expect("touch-prompt")
        tapSimulated("simulated-touch")
    }

    /// From Home: the web folder, then github.com with PIN and touch.
    private func openGitHubWithKey(askingForPin: Bool = true) {
        tap("folder-web")
        tap("entry-github.com")
        expectLabel("key-sheet-purpose", contains: "Open github.com")
        if askingForPin {
            enterPin(Self.pin)
        }
        touchTheKey()
        expectLabel("entry-title", contains: "github.com")
    }

    // MARK: Browse and search

    func testHomeListsTheFixtureFoldersWithTheirCounts() {
        for appearance in ["light", "dark"] {
            launch(appearance: appearance)
            expectLabel("folder-archive", contains: "archive, folder, 2 passwords")
            expectLabel("folder-bank", contains: "bank, folder, 3 passwords")
            expectLabel("folder-web", contains: "web, folder, 6 passwords")
            expectLabel("folder-servers", contains: "servers, folder, 3 passwords")
            expectAbsent("face-id-github.com", "Nothing opens with Face ID before the key was used")
            snapshot("home-\(appearance)")

            scrollTo("entry-Home Wi-Fi")
            expectLabel("entry-Home Wi-Fi", contains: "Home Wi-Fi, needs your key")
            expectLabel("entry-phone-pin", contains: "phone-pin, needs your key")
            snapshot("home-\(appearance)-bottom")
        }
    }

    func testOpeningAFolderShowsItsSubfoldersAndEntries() {
        launch()
        tap("folder-work")
        XCTAssertTrue(app.navigationBars["work"].waitForExistence(timeout: 10), "The title should be the folder name")
        expectLabel("entry-Jira", contains: "Jira, needs your key")
        expect("entry-payroll")
        expect("entry-vpn")
        expect("entry-wiki.example.com")
        snapshot("work")

        goBack()
        tap("folder-servers")
        expectLabel("folder-example.net", contains: "example.net, folder, 2 passwords")
        expect("entry-backup-box")
        tap("folder-example.net")
        expect("entry-deploy")
        expect("entry-root")
        snapshot("servers-example.net")
    }

    func testSearchFindsAMiddleTermAndSaysWhenNothingMatches() {
        for appearance in ["light", "dark"] {
            launch(appearance: appearance)
            let field = app.searchFields.firstMatch
            XCTAssertTrue(field.waitForExistence(timeout: 10), "Home should have a search field")
            field.tap()
            field.typeText("ycombinator")
            expectLabel("search-count", contains: "1 match")
            expectLabel("entry-news.ycombinator.com", contains: "news.ycombinator.com, web")
            snapshot("search-\(appearance)")

            field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 11) + "zzqx")
            let empty = app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "zzqx")).firstMatch
            XCTAssertTrue(empty.waitForExistence(timeout: 10), "No results should say what was searched for")
            XCTAssertTrue(empty.label.contains("No entry or folder called"), empty.label)
            snapshot("no-results-\(appearance)")

            field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 4))
            expect("folder-web")
        }
    }

    // MARK: Opening with the key

    func testOpeningAnEntryAsksForThePinThenATouchAndKeepsThePasswordHidden() {
        for appearance in ["light", "dark"] {
            launch(appearance: appearance)
            tap("folder-web")
            tap("entry-github.com")
            expectLabel("key-sheet-purpose", contains: "Open github.com")
            let field = pinField()
            expectAbsent("pin-tries-left", "A fresh key has all 3 tries, so none are counted out")
            expectAbsent("touch-prompt", "No touch before the PIN")
            if appearance == "light" { snapshot("key-sheet-pin") }
            field.tap()
            field.typeText(Self.pin)
            app.buttons["pin-continue"].tap()

            expect("touch-prompt")
            expectAbsent("entry-title", "Nothing opens before the key is touched")
            if appearance == "light" { snapshot("key-sheet-touch") }
            tapSimulated("simulated-touch")

            expectLabel("entry-title", contains: "github.com")
            expectGone("key-sheet-purpose")
            expectValue("password-value", "hidden")
            expectValue("field-Username", "you@example.com")
            expectValue("field-Website", "https://github.com/login")
            expectLabel("entry-notes", contains: "blue notebook")
            snapshot("entry-hidden-\(appearance)")

            app.buttons["password-reveal"].tap()
            expectValue("password-value", Self.githubPassword)
            snapshot("entry-revealed-\(appearance)")

            if appearance == "light" {
                app.buttons["copy-password"].tap()
                expectLabel("sim-last-toast", contains: "Password copied. Clears in 45 seconds.")
                snapshot("copied")
                app.buttons["show-password"].tap()
                expectValue("password-value", "hidden")
            }
        }
    }

    func testASecondEntryInTheSameRunDoesNotAskForThePinAgain() {
        launch()
        openGitHubWithKey()
        goBack()
        tap("entry-gitlab.com")
        expectLabel("key-sheet-purpose", contains: "Open gitlab.com")
        expect("touch-prompt")
        expectAbsent("pin-field", "The PIN is held for the run")
        snapshot("touch-only")
        tapSimulated("simulated-touch")
        expectLabel("entry-title", contains: "gitlab.com")
    }

    func testAWrongPinSaysHowManyTriesAreLeftAndOpensNothing() {
        launch()
        tap("folder-web")
        tap("entry-github.com")
        enterPin("000000")
        expectLabel("pin-tries-left", contains: "2 tries left")
        expect("pin-field")
        expectAbsent("touch-prompt", "A wrong PIN never reaches the touch")
        expectAbsent("entry-title", "A wrong PIN opens nothing")
        snapshot("wrong-pin")

        app.buttons["key-sheet-cancel"].tap()
        expectGone("key-sheet-purpose")
        expectAbsent("entry-title", "Cancel opens nothing")
    }

    func testCancelWhileWaitingForATouchOpensNothing() {
        launch(extra: ["-pawSimTouchTimeoutMs", "4000"])
        tap("folder-web")
        tap("entry-github.com")
        enterPin(Self.pin)
        expect("touch-prompt")
        app.buttons["key-sheet-cancel"].tap()
        expectGone("key-sheet-purpose")

        // Past the key's own timeout: the late answer must not open anything.
        sleep(6)
        expectAbsent("entry-title", "Nothing opens after a cancel")
        expectAbsent("key-sheet-purpose", "The sheet stays closed")
        snapshot("after-cancel")

        // The key is free again.
        tap("entry-gitlab.com")
        touchTheKey()
        expectLabel("entry-title", contains: "gitlab.com")
    }

    func testATouchAfterCancelOpensAndKeepsNothing() {
        launch()
        tap("folder-web")
        tap("entry-github.com")
        enterPin(Self.pin)
        expect("touch-prompt")
        app.buttons["key-sheet-cancel"].tap()
        expectGone("key-sheet-purpose")
        goBack()

        // The key is still blinking: touch it now, after the cancel.
        app.buttons["settings-button"].tap()
        app.buttons["sim-touch-key"].tap()
        app.buttons["settings-done"].tap()
        sleep(3)

        expectAbsent("entry-title", "A late touch opens nothing")
        expect("folder-web")
        expectAbsent("face-id-github.com", "A late touch keeps no Face ID copy")
        expectAbsent("recent-github.com", "A cancelled open is not a recent one")
        snapshot("home-after-late-touch")
    }

    func testLockNowWhileATouchIsPendingLeavesNoCopyAndForgetsThePin() {
        launch()
        tap("folder-web")
        tap("entry-github.com")
        enterPin(Self.pin)
        expect("touch-prompt")
        app.buttons["key-sheet-cancel"].tap()
        expectGone("key-sheet-purpose")
        goBack()

        // The key is still waiting for the touch when Lock now runs.
        app.buttons["settings-button"].tap()
        app.buttons["settings-lock-now"].tap()
        app.buttons["settings-button"].tap()
        app.buttons["sim-touch-key"].tap()
        app.buttons["settings-done"].tap()
        sleep(3)

        expectAbsent("entry-title", "A touch after Lock now opens nothing")
        expectAbsent("face-id-github.com", "A touch after Lock now keeps no Face ID copy")
        expectAbsent("recent-github.com", "A touch after Lock now is not a recent open")

        tap("folder-web")
        tap("entry-gitlab.com")
        expect("pin-field")
        snapshot("pin-after-late-touch")
    }

    // MARK: Face ID

    func testAnEntryOpenedWithTheKeyThenOpensWithFaceId() {
        launch()
        openGitHubWithKey()
        expectLabel("face-id-window-text", contains: "Opens with Face ID until")
        goBack()
        goBack()
        expectLabel("face-id-github.com", contains: "opens with Face ID")
        snapshot("home-face-id")

        tap("face-id-github.com")
        expectLabel("entry-title", contains: "github.com")
        expectAbsent("key-sheet-purpose", "Face ID opens without the key sheet")
        expectValue("password-value", "hidden")
        app.buttons["password-reveal"].tap()
        expectValue("password-value", Self.githubPassword)
    }

    func testCancelledFaceIdOpensNothing() {
        launch(extra: ["-pawSimFaceId", "cancel"])
        openGitHubWithKey()
        goBack()
        goBack()
        tap("face-id-github.com")
        expectLabel("warning-row", contains: "cancelled")
        expectAbsent("entry-title", "A cancelled Face ID opens nothing")
        expectAbsent("key-sheet-purpose", "A cancelled Face ID does not ask for the key by itself")
        snapshot("face-id-cancelled")
    }

    func testAnEntryNeedsTheKeyAgainOnceItsFaceIdTimeEnds() {
        launch()
        openGitHubWithKey()
        goBack()
        goBack()
        expect("face-id-github.com")

        app.buttons["settings-button"].tap()
        app.buttons["sim-advance-9h"].tap()
        app.buttons["settings-done"].tap()

        expectLabel("recent-github.com", contains: "needs your key")
        expectAbsent("face-id-github.com", "The copy has ended")
        snapshot("home-recent")

        tap("recent-github.com")
        expect("touch-prompt")
        expectAbsent("pin-field", "The PIN is still held")
        snapshot("touch-again")
    }

    func testLockNowEndsEveryFaceIdCopy() {
        launch()
        openGitHubWithKey()
        goBack()
        goBack()
        expect("face-id-github.com")

        app.buttons["settings-button"].tap()
        app.buttons["settings-lock-now"].tap()
        expectGone("face-id-github.com")
        expectLabel("recent-github.com", contains: "needs your key")

        tap("recent-github.com")
        expect("pin-field")
        snapshot("pin-after-lock")
    }

    // MARK: Another key

    func testAnEntryForAnotherKeySaysSoWithoutAskingForThePin() {
        launch()
        tap("folder-archive")
        expectLabel("entry-old-forum", contains: "Encrypted for another key")
        expectLabel("entry-university", contains: "Encrypted for another key")
        snapshot("archive")

        tap("entry-old-forum")
        expectLabel("another-key-explanation", contains: "key ID")
        expectAbsent("pin-field", "No PIN for a key that cannot open it")
        expectAbsent("key-sheet-purpose", "No key sheet for a key that cannot open it")
        expectLabel("another-key-footer", contains: "a computer that has that key")
        snapshot("another-key")
    }

    // MARK: The key session

    func testTriesLeftAreShownBeforeAnythingIsTyped() {
        // Another computer has used two of the key's three tries.
        launch(extra: ["-pawSimPinTries", "1"])
        tap("folder-web")
        tap("entry-github.com")
        pinField()
        expectLabel("pin-tries-left", contains: "1 try left before the key locks.")
        expectLabel("pin-continue", contains: "Use last try")
        expectLabel("pin-show", contains: "Hide PIN")
        snapshot("last-try")
        enterPin(Self.pin)
        touchTheKey()
        expectLabel("entry-title", contains: "github.com")
    }

    func testAShortPinIsRefusedWithoutUsingATry() {
        launch()
        tap("folder-web")
        tap("entry-github.com")
        let field = pinField()
        field.tap()
        field.typeText("12345")
        XCTAssertFalse(app.buttons["pin-continue"].isEnabled, "Continue waits for 6 characters")
        // The return key sends it anyway; the core refuses it before the card.
        field.typeText("\n")
        expectLabel("key-sheet-headline", contains: "A PIN has 6 to 127 characters")
        expectLabel("key-sheet-body", contains: "no try was used")
        expectAbsent("pin-tries-left", "The key still has all 3 tries")
        expectAbsent("touch-prompt", "A refused PIN never reaches the touch")
        snapshot("short-pin")

        enterPin(Self.pin)
        touchTheKey()
        expectLabel("entry-title", contains: "github.com")
    }

    func testAKeyPluggedInAfterTheSheetOpensCarriesOn() {
        launch(extra: ["-pawSimUnplugged", "1"])
        tap("folder-web")
        tap("entry-github.com")
        expectLabel("key-sheet-headline", contains: "Plug in your key")
        expectAbsent("pin-field", "No PIN before a key is plugged in")
        snapshot("plug-in")

        tapSimulated("simulated-plug-in")
        enterPin(Self.pin)
        touchTheKey()
        expectLabel("entry-title", contains: "github.com")
    }

    func testThePinIsForgottenAfter15MinutesInTheBackground() {
        // Each trip to the background moves the simulated clock 16 minutes on.
        launch(extra: ["-pawSimAwayMs", String(16 * 60 * 1000)])
        openGitHubWithKey()
        goBack()

        XCUIDevice.shared.press(.home)
        sleep(2)
        app.activate()
        tap("entry-gitlab.com")
        expectLabel("key-sheet-purpose", contains: "Open gitlab.com")
        pinField()
        expectLabel("key-sheet-body", contains: "paw forgot your PIN after 15 minutes away.")
        snapshot("pin-forgotten")

        enterPin(Self.pin)
        touchTheKey()
        expectLabel("entry-title", contains: "gitlab.com")
    }

    func testATouchTimeoutCanBeTriedAgainWithoutThePin() {
        // Long enough to photograph the second touch stage before it ends too.
        launch(extra: ["-pawSimTouchTimeoutMs", "8000"])
        tap("folder-web")
        tap("entry-github.com")
        enterPin(Self.pin)
        expect("touch-prompt")

        expectLabel("key-sheet-headline", contains: "Your key wasn't touched in time", timeout: 20)
        expectLabel("key-sheet-body", contains: "Nothing was opened.")
        expectAbsent("entry-title", "Nothing opens without a touch")
        snapshot("touch-timeout")

        app.buttons["key-sheet-retry"].tap()
        expect("touch-prompt")
        expectAbsent("pin-field", "The PIN is still held after a touch timeout")
        snapshot("touch-again")
        tapSimulated("simulated-touch")
        expectLabel("entry-title", contains: "github.com")
    }

    func testARevealedPasswordHidesWhenPawLeavesTheScreen() {
        launch()
        openGitHubWithKey()
        app.buttons["password-reveal"].tap()
        expectValue("password-value", Self.githubPassword)

        // Notification Center over paw: paw is inactive and the cover is up.
        let top = app.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.001))
        let middle = app.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.6))
        top.press(forDuration: 0.1, thenDragTo: middle)
        expect("privacy-cover")
        snapshot("cover")

        // Then to the background and back.
        XCUIDevice.shared.press(.home)
        sleep(2)
        app.activate()
        expectGone("privacy-cover")
        expectValue("password-value", "hidden")
        snapshot("hidden-after-background")
    }

    func testLargeTypeShowsThePasswordInGroups() {
        launch()
        openGitHubWithKey()
        app.buttons["password-reveal"].tap()
        expectValue("password-value", Self.githubPassword)
        app.buttons["password-large-type"].tap()

        expect("large-type-group-1")
        expect("large-type-group-2")
        expectLabel("large-type-cell-1", contains: "Lowercase t, 1 of 21")
        expectLabel("large-type-cell-6", contains: "Hyphen, 6 of 21")
        snapshot("large-type")

        app.buttons["large-type-done"].tap()
        expectGone("large-type-group-1")
    }
}
