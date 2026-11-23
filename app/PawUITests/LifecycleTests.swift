import XCTest

/// What happens to opened entries, Face ID copies, the clipboard and the
/// key over time and around the app's life, on the fixture store: an open
/// entry closes after 2 minutes away, when the iPhone locks and when its
/// copy ends; one entry's copy can be ended alone; a late-finishing open
/// removes only its own copy; a key whose first read fails is read again
/// without a replug. Time is the simulated clock's.
final class LifecycleTests: PawUITestCase {
    // MARK: Closing an open entry

    func testAnOpenEntryStaysUnderTwoMinutesAwayAndClosesAfter() {
        // Each trip to the background moves the simulated clock 1 minute on.
        launchFixture(extra: ["-pawSimAwayMs", String(60 * 1000)])
        openGitHubWithKey()
        app.buttons["password-reveal"].tap()
        expectValue("password-value", Self.githubPassword)
        backgroundAndReturn()
        expectLabel("entry-title", contains: "github.com")
        expectValue("password-value", "hidden")
        expectAbsent("entry-closed", "One minute away keeps the entry open")

        // 3 minutes away this time.
        app.terminate()
        launchFixture(extra: ["-pawSimAwayMs", String(3 * 60 * 1000)])
        openGitHubWithKey()
        backgroundAndReturn()
        expectLabel("entry-closed", contains: "Closed after 2 minutes away")
        expectAbsent("password-value", "A closed entry shows nothing of it")
        snapshot("closed-away")

        // One tap opens it again: it still has its Face ID copy.
        tap("entry-reopen")
        expectLabel("entry-title", contains: "github.com")
        expectAbsent("key-sheet-purpose", "It opens with Face ID")
    }

    func testAnOpenEntryClosesWhenTheIPhoneLocks() {
        launchFixture()
        openGitHubWithKey()
        tap("sim-entry-device-lock")
        expectLabel("entry-closed", contains: "Closed when your iPhone locked")
        expectAbsent("password-value", "A closed entry shows nothing of it")
        snapshot("closed-locked")
    }

    func testAnOpenEntryClosesWhenItsFaceIdTimeEnds() {
        launchFixture()
        openGitHubWithKey()
        expectLabel("face-id-window-text", contains: "Opens with Face ID until")
        tap("sim-entry-advance-9h")
        expectLabel("entry-closed", contains: "Its Face ID time ended")
        expectAbsent("password-value", "A closed entry shows nothing of it")
        snapshot("closed-expired")

        // Reopening needs the key again; the PIN is still held.
        tap("entry-reopen")
        expectLabel("key-sheet-purpose", contains: "Open github.com")
        touchTheKey()
        expectLabel("entry-title", contains: "github.com")
    }

    // MARK: One entry's copy

    func testLockThisPasswordEndsOnlyItsOwnCopy() {
        launchFixture()
        openGitHubWithKey()
        goBack()
        tap("entry-gitlab.com")
        touchTheKey()
        expectLabel("entry-title", contains: "gitlab.com")
        expectLabel("lock-now", contains: "Lock this password")
        snapshot("lock-this-password")
        tap("lock-now")

        expectGone("entry-title")
        expectLabel("entry-gitlab.com", contains: "needs your key")
        expectLabel("entry-github.com", contains: "opens with Face ID")
        goBack()
        expect("face-id-github.com")
        expectAbsent("face-id-gitlab.com", "gitlab.com's copy ended")
    }

    func testAnOpenThatFinishesAfterCancelRemovesOnlyItsOwnCopy() {
        launchFixture()
        openGitHubWithKey()
        goBack()
        // The next open finishes past the point where Cancel can stop it,
        // as if Cancel came just as the card answered.
        goBack()
        inSettings(tapping: "sim-late-cancel-next")
        tap("folder-web")
        tap("entry-gitlab.com")
        touchTheKey()
        expectGone("key-sheet-purpose")
        waitForKeyTasksToEnd()

        expectAbsent("entry-title", "A late finish after Cancel opens nothing")
        goBack()
        expect("face-id-github.com")
        expectAbsent("face-id-gitlab.com", "The late open's copy was removed")
        snapshot("late-cancel")
    }

    // MARK: The clipboard

    func testCopyPutsThePasswordOnThePasteboardAndLockNowAndExpiryClearIt() {
        // The system's own 45-second expiry runs on real time, which a UI
        // test spends freely; paw's own runs on the simulated clock.
        launchFixture(extra: ["-pawSimPasteboardSecs", "3600"])
        openGitHubWithKey()
        app.buttons["copy-password"].tap()
        expectLabel("sim-last-toast", contains: "Password copied")
        goBack()
        goBack()
        XCTAssertEqual(pasteboardContents(), Self.githubPassword, "The password should be on the pasteboard")

        openSettings()
        tap("settings-lock-now")
        expectGone("settings-done")
        XCTAssertEqual(pasteboardContents(), "", "Lock now clears what paw put on the pasteboard")

        // Copied again, then 45 seconds pass on the simulated clock.
        openGitHubWithKey()
        app.buttons["copy-password"].tap()
        expectLabel("sim-last-toast", contains: "Password copied")
        goBack()
        goBack()
        inSettings(tapping: "sim-advance-1h")
        XCTAssertEqual(pasteboardContents(), "", "An expired copy is cleared")
    }

    // MARK: The key

    func testAKeyWhoseReadsFailRecoversWithoutAReplug() {
        // The key is in but doesn't answer when read, however often paw
        // tries, until the simulator lets it.
        let failing = ["-pawSimAttachFails", "1000"]
        launchFixture(extra: failing)
        tap("folder-web")
        tap("entry-github.com")
        expectLabel("key-sheet-headline", contains: "paw couldn't read your key")
        expectLabel("key-sheet-body", contains: "Try again")
        expectAbsent("pin-field", "No PIN before the key is read")
        snapshot("read-failed")
        // Still failing: Try again says so again, without a PIN.
        tap("key-sheet-retry")
        expectLabel("key-sheet-headline", contains: "paw couldn't read your key")
        // The key answers now: Try again reads it, no replug.
        tapSimulated("simulated-key-answers")
        tap("key-sheet-retry")
        enterPin(Self.pin)
        touchTheKey()
        expectLabel("entry-title", contains: "github.com")

        // The read at launch failed; by the time an entry is opened the key
        // answers, and opening reads it again by itself.
        app.terminate()
        launchFixture(extra: failing)
        inSettings(tapping: "sim-key-answers")
        tap("folder-web")
        tap("entry-github.com")
        pinField()
        expectAbsent("key-sheet-retry", "Opening read the key again by itself")
    }
}
