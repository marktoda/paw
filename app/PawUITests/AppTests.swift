import XCTest

/// The real screens on the fixture store (`-pawFixture 1`): 27 entries the
/// simulated key can open and 2 under `archive/` it cannot, a simulated key
/// with PIN 123456, a simulated Face ID and a simulated clock.
final class AppTests: PawUITestCase {
    private func launch(appearance: String = "light", extra: [String] = []) {
        launchFixture(appearance: appearance, extra: extra)
    }

    // MARK: Browse and search

    func testHomeListsTheFixtureFoldersWithTheirCounts() {
        launch()
        expectLabel("folder-archive", contains: "archive, folder, 2 passwords")
        expectLabel("folder-bank", contains: "bank, folder, 3 passwords")
        expectLabel("folder-web", contains: "web, folder, 6 passwords")
        expectLabel("folder-servers", contains: "servers, folder, 3 passwords")
        expectAbsent("face-id-github.com", "Nothing opens with Face ID before the key was used")
        snapshot("home")

        scrollTo("entry-Home Wi-Fi")
        expectLabel("entry-Home Wi-Fi", contains: "Home Wi-Fi, needs your key")
        expectLabel("entry-phone-pin", contains: "phone-pin, needs your key")
        snapshot("home-bottom")
    }

    func testOpeningAFolderShowsItsSubfoldersAndEntries() {
        launch()
        tap("folder-work")
        XCTAssertTrue(waitToExist(app.navigationBars["work"], timeout: Self.patience), "The title should be the folder name")
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
        launch()
        let field = app.searchFields.firstMatch
        XCTAssertTrue(waitToExist(field, timeout: Self.patience), "Home should have a search field")
        focus(field)
        field.typeText("ycombinator")
        expectLabel("search-count", contains: "1 match")
        expectLabel("entry-news.ycombinator.com", contains: "news.ycombinator.com, web")
        snapshot("search")

        field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 11) + "zzqx")
        let empty = app.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "zzqx")).firstMatch
        XCTAssertTrue(waitToExist(empty, timeout: Self.patience), "No results should say what was searched for")
        XCTAssertTrue(empty.label.contains("No password or folder called"), empty.label)
        snapshot("no-results")

        field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 4))
        expect("folder-web")
    }

    // MARK: Opening with the key

    func testOpeningAnEntryAsksForThePinThenATouchAndKeepsThePasswordHidden() {
        launch()
        tap("folder-web")
        tap("entry-github.com")
        expectLabel("key-sheet-purpose", contains: "Open github.com")
        let field = pinField()
        expectAbsent("pin-tries-left", "A fresh key has all 3 tries, so none are counted out")
        expectAbsent("touch-prompt", "No touch before the PIN")
        snapshot("key-sheet-pin")
        enterPin(Self.pin, send: false)
        expectEnabled("pin-continue", true)
        tapUntilGone("pin-continue")

        expect("touch-prompt")
        expectAbsent("entry-title", "Nothing opens before the key is touched")
        snapshot("key-sheet-touch")
        tapSimulated("simulated-touch")

        expectLabel("entry-title", contains: "github.com")
        expectGone("key-sheet-purpose")
        expectValue("password-value", "hidden")
        expectValue("field-Username", "you@example.com")
        expectValue("field-Website", "https://github.com/login")
        expectLabel("entry-notes", contains: "blue notebook")
        snapshot("entry-hidden")

        app.buttons["password-reveal"].tap()
        expectValue("password-value", Self.githubPassword)
        snapshot("entry-revealed")

        app.buttons["copy-password"].tap()
        expectLabel("sim-last-toast", contains: "Password copied. Clears in 45 seconds.")
        snapshot("copied")
        app.buttons["show-password"].tap()
        expectValue("password-value", "hidden")
    }

    func testASecondEntryInTheSameRunDoesNotAskForThePinAgain() {
        launch()
        openGitHubWithKey()
        goBack()
        tap("entry-gitlab.com")
        expectLabel("key-sheet-purpose", contains: "Open gitlab.com")
        expect("touch-prompt")
        expectAbsent("pin-field", "The PIN is held for the run")
        // The PIN chip reads as done ("PIN correct"), not skipped.
        expectLabel("key-sheet-steps", contains: "Step 3 of 3: Touch. Done: Plugged in, PIN correct")
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

    // MARK: Face ID

    /// The largest size before the bar stacks: "Lock" stays beside the time.
    func testTheFaceIdBarAtTheLargestStandardTextSize() {
        launch(extra: ["-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryXXXL"])
        openGitHubWithKey()
        expectLabel("face-id-window-text", contains: "Opens with Face ID until")
        snapshot("entry-face-id-xxxl")
    }

    func testAnEntryOpenedWithTheKeyThenOpensWithFaceId() {
        launch()
        openGitHubWithKey()
        expectLabel("face-id-window-text", contains: "Opens with Face ID until")
        expectLabel("lock-now", contains: "Lock this password")
        snapshot("entry-face-id")
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

        inSettings(tapping: "sim-advance-9h")

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
        enterPin("12345", send: false)
        expectEnabled("pin-continue", false)
        // The return key sends it anyway; the core refuses it before the card.
        pinField().typeText("\n")
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

        backgroundAndReturn()
        tap("entry-gitlab.com")
        expectLabel("key-sheet-purpose", contains: "Open gitlab.com")
        pinField()
        expectLabel("key-sheet-body", contains: "Asked again after 15 minutes away.")
        snapshot("pin-forgotten")

        enterPin(Self.pin)
        touchTheKey()
        expectLabel("entry-title", contains: "gitlab.com")
    }

    func testATouchTimeoutCanBeTriedAgainWithoutThePin() {
        launch()
        tap("folder-web")
        tap("entry-github.com")
        enterPin(Self.pin)
        // The key's timeout runs out now, by the simulator's control.
        timeOutTheTouch()

        expectLabel("key-sheet-headline", contains: "Your key wasn't touched in time")
        expectLabel("key-sheet-body", contains: "touch the gold part while it blinks")
        expectAbsent("entry-title", "Nothing opens without a touch")
        snapshot("touch-timeout")

        app.buttons["key-sheet-retry"].tap()
        expect("touch-prompt")
        expectAbsent("pin-field", "The PIN is still held after a touch timeout")
        snapshot("touch-again")
        tapSimulated("simulated-touch")
        expectLabel("entry-title", contains: "github.com")
    }

    /// The cover is up only while paw is away; `-pawSimPrivacyCover` holds
    /// it up to take its picture.
    func testThePrivacyCoverShowsOnlyTheName() {
        for appearance in ["light", "dark"] {
            launch(appearance: appearance, extra: ["-pawSimPrivacyCover", "1"])
            expectLabel("privacy-cover", contains: "paw is hidden")
            snapshot("privacy-cover-\(appearance)")
        }
    }

    func testARevealedPasswordHidesWhenPawLeavesTheScreen() {
        launch()
        openGitHubWithKey()
        app.buttons["password-reveal"].tap()
        expectValue("password-value", Self.githubPassword)
        let cover = expect("sim-privacy-cover-shown")
        let before = Int((cover.value as? String) ?? "") ?? 0

        // To the background and back. On the way out paw is inactive first,
        // and the cover goes up then, before iOS takes the app switcher's
        // picture. A test can't see it while paw is away; paw counts it.
        backgroundAndReturn()
        wait(for: "value != '\(before)'", on: cover, timeout: Self.patience,
             "The privacy cover should have been drawn while paw was away")
        expectGone("privacy-cover")
        // Still masked, and the entry is still open (well inside 2 minutes).
        expectValue("password-value", "hidden")
        expectLabel("entry-title", contains: "github.com")
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

        // Large Type closes when paw goes to the background.
        app.buttons["password-large-type"].tap()
        expect("large-type-group-1")
        backgroundAndReturn()
        expectGone("large-type-group-1")
        expectValue("password-value", "hidden")
    }

    // MARK: Key log

    /// A line of Settings › Key diagnostics' key log that contains `text`,
    /// scrolled to: the list builds its rows lazily.
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

    func testTheKeyLogShowsTheCommandsOfAnOpenAndCopyPutsItOnThePasteboard() {
        launch(extra: ["-pawSimPasteboardSecs", "3600"])
        openGitHubWithKey()
        goBack()
        goBack()
        openSettings()
        tap("settings-key-diagnostics")
        // Copy before looking down the list: once scrolled, the button can
        // sit under the navigation bar, where a tap lands on the bar.
        tap("key-log-copy")
        expectLabel("key-log-copied", contains: "Clears in 10 minutes")
        XCTAssertTrue(keyLogLine(containing: "PSO:DECIPHER: 9000 ok").exists, "The log should show the decryption")
        XCTAssertTrue(keyLogLine(containing: "VERIFY 00 82 (with PIN): 9000 ok").exists,
                      "The log should show the PIN being checked, without the PIN")
        snapshot("key-log")
        app.buttons["diagnostics-done"].tap()
        expectGone("diagnostics-done")
        closeSettings()

        let copied = pasteboardContents()
        XCTAssertTrue(copied.contains("PSO:DECIPHER: 9000 ok"), copied)
        XCTAssertTrue(copied.contains("task open: PIN held no"), copied)
        for secret in [Self.pin, Self.githubPassword, "github.com"] {
            XCTAssertFalse(copied.contains(secret), "The key log must not hold \(secret)")
        }
    }
}
