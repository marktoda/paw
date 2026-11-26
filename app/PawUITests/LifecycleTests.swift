import XCTest

/// What happens to opened entries, Face ID copies and the clipboard over
/// time and around the app's life, on the fixture store: an open entry
/// closes after 2 minutes away; one entry's copy can be ended alone; the
/// clipboard is cleared. Time is the simulated clock's. Closing when the
/// iPhone locks or a copy ends, a late-finishing open and a key whose
/// reads fail are tested on the model (app/PawTests/ClosingTests.swift,
/// KeyOpenTests.swift).
final class LifecycleTests: PawUITestCase {
    // MARK: Closing an open entry

    /// That under two minutes away keeps it open is ClosingTests.
    func testAnOpenEntryClosesAfterTwoMinutesAwayAndReopensWithFaceId() {
        // A trip to the background moves the simulated clock 3 minutes on.
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

    // MARK: The clipboard

    func testCopyPutsThePasswordOnThePasteboardAndLockNowAndExpiryClearIt() {
        // The system's own 45-second expiry runs on real time, which a UI
        // test spends freely; paw's own runs on the simulated clock.
        launchFixture(extra: ["-pawSimPasteboardSecs", "3600"])
        openGitHubWithKey()
        tapWhenHittable(app.buttons["copy-password"], "Copy password")
        expectLabel("sim-last-toast", contains: "Password copied")
        goBack()
        goBack()
        XCTAssertEqual(pasteboardContents(), Self.githubPassword, "The password should be on the pasteboard")

        openSettings()
        tap("settings-lock-now")
        expectGone("settings-done")
        XCTAssertEqual(pasteboardContents(), "", "Lock now clears what paw put on the pasteboard")
        expectLabel("recent-github.com", contains: "needs your key")
        expectGone("face-id-github.com")

        // Copied again, then 45 seconds pass on the simulated clock.
        openGitHubWithKey()
        tapWhenHittable(app.buttons["copy-password"], "Copy password")
        expectLabel("sim-last-toast", contains: "Password copied")
        goBack()
        goBack()
        inSettings(tapping: "sim-advance-1h")
        XCTAssertEqual(pasteboardContents(), "", "An expired copy is cleared")
    }
}
