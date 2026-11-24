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
            XCTAssertTrue(empty.label.contains("No password or folder called"), empty.label)
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
            enterPin(Self.pin, send: false)
            expectEnabled("pin-continue", true)
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
        launch()
        tap("folder-web")
        tap("entry-github.com")
        enterPin(Self.pin)
        expect("touch-prompt")
        app.buttons["key-sheet-cancel"].tap()
        expectGone("key-sheet-purpose")

        // The key is still waiting for the touch the sheet let go of. Its
        // timeout runs out now; the late answer must not open anything.
        goBack()
        inSettings(tapping: "sim-time-out-touch")
        waitForKeyTasksToEnd()
        expectAbsent("entry-title", "Nothing opens after a cancel")
        expectAbsent("key-sheet-purpose", "The sheet stays closed")
        expectAbsent("face-id-github.com", "A cancelled open keeps no Face ID copy")
        snapshot("after-cancel")

        // The key is free again.
        tap("folder-web")
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
        inSettings(tapping: "sim-touch-key")
        waitForKeyTasksToEnd()

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
        openSettings()
        tap("settings-lock-now")
        expectGone("settings-done")
        inSettings(tapping: "sim-touch-key")
        waitForKeyTasksToEnd()

        expectAbsent("entry-title", "A touch after Lock now opens nothing")
        expectAbsent("face-id-github.com", "A touch after Lock now keeps no Face ID copy")
        expectAbsent("recent-github.com", "A touch after Lock now is not a recent open")

        tap("folder-web")
        tap("entry-gitlab.com")
        expect("pin-field")
        snapshot("pin-after-late-touch")
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
}
