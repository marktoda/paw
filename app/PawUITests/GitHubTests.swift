import XCTest

/// "Choose from GitHub" in setup, Settings › GitHub and checking for
/// changes with the sign-in, against the core's simulated GitHub
/// (`-pawSimGitHub <mode>`), which serves the simulated git host's store as
/// `mira/password-store` and shows the code `WDJB-MJHT`. Nothing here talks
/// to the real GitHub. The simulated GitHub asks paw to wait 5 seconds
/// between polls, so a sign-in takes a few seconds after "authorise".
final class GitHubTests: PawUITestCase {
    private static let store = "github-repo-mira/password-store"

    // MARK: Helpers

    private func launch(_ mode: String = "signedOut", appearance: String = "light", extra: [String] = []) {
        app.launchArguments = ["-pawSetup", "1", "-pawAppearance", appearance, "-pawSimGitHub", mode]
            + keyArguments() + extra
        app.launch()
    }

    /// Welcome, "Your key", then the address screen.
    private func toAddress() {
        expect("setup-welcome")
        tap("setup-start")
        expect("setup-your-key")
        tapUntilGone("key-continue")
        expect("repo-address")
    }

    /// "Choose from GitHub": the code shows.
    private func openPicker() {
        tap("address-github")
        expectText("github-code", contains: "WDJB-MJHT")
        expect("github-waiting")
    }

    /// The person enters the code on GitHub: the list follows.
    private func authorise() {
        tap("sim-github-authorise")
        expect("github-repositories", timeout: Self.patience)
    }

    /// From a chosen repository to Home: the host check, the download with
    /// the PIN, the key check with a touch.
    private func finishSetup(file: StaticString = #filePath, line: UInt = #line) {
        expect("setup-confirm-host", file: file, line: line)
        tap("host-trust")
        expect("setup-download")
        tap("download-start")
        enterPin(Self.pin)
        touchTheKey()
        expect("setup-check", timeout: 60)
        tap("check-start")
        touchTheKey()
        expect("setup-done")
        expectText("done-count", contains: "29 passwords")
        tap("done-open")
        expect("home-title")
    }

    /// Setup through GitHub, to Home.
    private func setUpThroughGitHub() {
        toAddress()
        openPicker()
        authorise()
        tap(Self.store)
        finishSetup()
    }

    /// Starts a check from Home: the pill starts it when nothing has been
    /// checked yet, else it opens the sync sheet, whose button starts it.
    /// Returns whether the sheet was used (and left open).
    @discardableResult
    private func checkFromHome() -> Bool {
        let pill = expect("sync-pill")
        let direct = pill.label.contains("Not checked yet") || pill.label.contains("need your key")
        pill.tap()
        if direct { return false }
        expect("sync-sheet")
        tap("sync-check")
        return true
    }

    /// Settings builds its rows lazily: scroll to the row, then read it.
    private func inSettingsList(_ identifier: String, contains text: String,
                                file: StaticString = #filePath, line: UInt = #line) {
        scrollTo(identifier, file: file, line: line)
        expectText(identifier, contains: text, file: file, line: line)
    }

    // MARK: Signing in and choosing

    func testSigningInWithTheCodeAndChoosingTheStoreFinishesSetupWithOnePin() {
        launch()
        toAddress()
        snapshot("address")
        openPicker()
        snapshot("code")
        // In the simulator "Open" only copies: the simulated GitHub has no page.
        tap("github-open")
        expectText("github-copied", contains: "Code copied")
        snapshot("code-copied")
        authorise()
        expectText(Self.store, contains: "private")
        snapshot("list")
        tap(Self.store)
        expectGone("github-repositories")
        finishSetup()
        expectText("folder-web", contains: "web, folder, 6 passwords")
        XCTAssertEqual(pasteboardContents(), "WDJB-MJHT", "Open copies the code")
    }

    func testADeclinedCodeRecoversWithTryAgain() {
        launch("denied")
        toAddress()
        tap("address-github")
        expectText("github-ended", contains: "You declined on GitHub")
        snapshot("denied")
        tap("github-try-again")
        expectText("github-code", contains: "WDJB-MJHT")
        authorise()
        expect(Self.store)
    }

    func testAnExpiredCodeRecoversWithTryAgain() {
        launch("expired")
        toAddress()
        tap("address-github")
        expectText("github-ended", contains: "The code expired")
        snapshot("expired")
        tap("github-try-again")
        expectText("github-code", contains: "WDJB-MJHT")
        authorise()
        expect(Self.store)
    }

    func testCancellingTheCodeRecoversWithTryAgain() {
        launch()
        toAddress()
        openPicker()
        tap("github-cancel")
        expectText("github-ended", contains: "Sign-in cancelled")
        expectAbsent("github-waiting", "A cancelled sign-in should stop waiting")
        snapshot("cancelled")
        tap("github-try-again")
        expectText("github-code", contains: "WDJB-MJHT")
        authorise()
        expect(Self.store)
    }

    func testNotInstalledShowsTheLinkAndCheckAgainFindsTheStore() {
        launch("notInstalled")
        toAddress()
        openPicker()
        tap("sim-github-authorise")
        expectText("github-not-installed", contains: "can't see any of your repositories")
        expect("github-install-link")
        snapshot("not-installed")
        tap("sim-github-install")
        tap("github-check-again")
        expect(Self.store)
    }

    func testALongListIsSearchableWithTheStoreFirst() {
        launch("many")
        toAddress()
        openPicker()
        authorise()
        let rows = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'github-repo-'"))
        XCTAssertTrue(rows.firstMatch.waitForExistence(timeout: Self.patience), "The list should show")
        XCTAssertEqual(rows.element(boundBy: 0).identifier, Self.store, "The store should be listed first")
        snapshot("many")
        let search = app.searchFields.firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: Self.patience), "A long list should be searchable")
        search.tap()
        search.typeText("other-0007")
        expect("github-repo-mira/other-0007")
        expectGone(Self.store)
        snapshot("searched")
        // Not chosen: its address is github.com's own, and tests never go there.
    }

    // MARK: Checking for changes with the sign-in

    func testCheckingWhileSignedInAsksForNoKeyAndBringsInARemoteChange() {
        launch(extra: ["-pawSimRemoteChange", "add"])
        setUpThroughGitHub()
        if checkFromHome() {
            tap("sync-sheet-done")
        }
        expectLabel("sync-summary-title", contains: "1 password changed")
        expect("sync-added-forum.example")
        waitForKeyTasksToEnd()
        expectAbsent("key-sheet-purpose", "Checking with the GitHub sign-in should never ask for the key")
        expectAbsent("pin-field", "Checking with the GitHub sign-in should never ask for the PIN")
        snapshot("summary")
        tapWhenHittable(expect("sync-pill"), "The sync pill")
        expect("sync-sheet")
        let check = expect("sync-check")
        XCTAssertFalse(check.label.contains("Needs your key"), "Checking with the sign-in has no key disc")
        snapshot("sync-sheet-signed-in")
    }

    func testASignInRemovedOnGitHubFallsBackToTheKey() {
        launch(extra: ["-pawSimRemoteChange", "add"])
        setUpThroughGitHub()
        openSettings()
        tap("sim-github-revoke")
        closeSettings()
        checkFromHome()
        expect("touch-prompt")
        expectText("key-sheet-note", contains: "Your GitHub sign-in ended")
        snapshot("fallback")
        tapSimulated("simulated-touch")
        expectLabel("sync-summary-title", contains: "1 password changed")
        openSettings()
        inSettingsList("settings-github-sign-in", contains: "Sign in to GitHub")
        closeSettings()
    }

    func testSigningOutInSettingsMakesTheCheckNeedTheKeyAgain() {
        launch()
        setUpThroughGitHub()
        openSettings()
        inSettingsList("settings-github-status", contains: "mira/password-store")
        snapshot("settings-signed-in")
        tap("settings-github-sign-out")
        scrollTo("settings-github-sign-out-warning")
        scrollTo("settings-github-sign-out-revoke")
        tap("settings-github-sign-out-confirm")
        inSettingsList("settings-github-sign-in", contains: "Sign in to GitHub")
        expectAbsent("settings-github-signed-out", "A plain sign-out leaves just the sign-in button")
        snapshot("settings-signed-out")
        closeSettings()
        let pill = expect("sync-pill")
        if pill.label.contains("need your key") {
            pill.tap()
        } else {
            pill.tap()
            expectLabel("sync-check", contains: "Needs your key")
            tap("sync-check")
        }
        expectLabel("key-sheet-purpose", contains: "Update your store")
        tap("key-sheet-cancel")
        expectGone("key-sheet-purpose")
        // Signing in again from Settings finds the store's repository by itself.
        openSettings()
        tap("settings-github-sign-in")
        expectText("github-code", contains: "WDJB-MJHT")
        tap("sim-github-authorise")
        expectGone("github-code")
        inSettingsList("settings-github-status", contains: "mira/password-store")
        closeSettings()
    }
}
