import XCTest

/// First-run setup (`-pawSetup 1`) against the simulated key (PIN 123456,
/// a touch for every use) and the core's simulated git host, which runs
/// inside the app and serves the fixture store: 29 passwords, 27 of them
/// for the simulated key. The host's address is
/// `ssh://git@127.0.0.1:<port>/store.git`.
final class SetupTests: PawUITestCase {
    // MARK: Helpers

    /// `keep` keeps what earlier launches set up; otherwise setup starts
    /// from nothing. The key's touch timeout is the long one: a test about
    /// the timeout ends it with `timeOutTheTouch()`.
    private func launch(appearance: String = "light", remote: String? = nil, keep: Bool = false,
                        extra: [String] = []) {
        var arguments = ["-pawSetup", "1", "-pawAppearance", appearance] + keyArguments()
        if keep { arguments += ["-pawSetupKeep", "1"] }
        if let remote { arguments += ["-pawSimRemote", remote] }
        app.launchArguments = arguments + extra
        app.launch()
    }

    /// Welcome, "Your key", then the address screen.
    private func toAddress(shots: Bool = false) {
        expect("setup-welcome")
        if shots { snapshot("welcome") }
        tap("setup-start")
        expect("setup-your-key")
        expectText("key-serial", contains: "serial")
        if shots { snapshot("your-key") }
        tap("key-continue")
        expect("repo-address")
    }

    /// Types the simulated host's address and trusts its key.
    private func toDownload(shots: Bool = false) {
        tap("simulated-address")
        expectText("address-reading", contains: "Connects to 127.0.0.1 as git")
        if shots { snapshot("address") }
        tap("address-connect")
        expect("setup-confirm-host")
        expect("host-fingerprint")
        if shots { snapshot("confirm-host") }
        tap("host-trust")
        expect("setup-download")
        if shots { snapshot("download") }
    }

    /// Download: the PIN once (unless held), a touch for the login.
    private func download(pin: Bool = true, shots: Bool = false) {
        tap("download-start")
        if pin {
            enterPin(Self.pin)
        }
        expect("touch-prompt")
        if shots { snapshot("download-touch") }
        tapSimulated("simulated-touch")
        if shots {
            // What follows the touch: the clone with its progress ("412 of
            // 1,240 · 1.2 MB"), or Done if the fixture has already arrived.
            snapshot("download-progress")
        }
        expect("setup-check", timeout: 60)
    }

    /// The key check: no PIN (it is held), one touch.
    private func check(shots: Bool = false) {
        if shots { snapshot("check") }
        tap("check-start")
        expect("touch-prompt")
        expectAbsent("pin-field", "The PIN typed for the download should still be held")
        touchTheKey()
        expect("setup-done")
    }

    private func setUpFully(shots: Bool = false) {
        toAddress(shots: shots)
        toDownload(shots: shots)
        download(shots: shots)
        check(shots: shots)
        expectText("done-count", contains: "29 passwords")
        if shots { snapshot("done") }
        tap("done-open")
        expect("home-title")
    }

    /// Empties the address field: the cursor goes to the end, then one
    /// delete per character.
    private func clearAddress() {
        let field = expect("repo-address")
        field.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.9)).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: Self.patience),
                      "The keyboard should come up for the address")
        if let typed = field.value as? String, typed != "git@github.com:you/password-store.git" {
            field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: typed.count))
        }
    }

    // MARK: The whole way

    func testAFullSetupAsksForThePinOnceAndEndsOnHomeWithTheStoresFolders() {
        launch()
        setUpFully(shots: true)
        expectText("folder-bank", contains: "bank, folder, 3 passwords")
        expectText("folder-web", contains: "web, folder, 6 passwords")
        snapshot("home")
    }

    func testAfterSetupAnEntryOpensWithoutAskingForThePinAgain() {
        launch()
        setUpFully()
        tap("folder-web")
        tap("entry-github.com")
        expect("touch-prompt")
        expectAbsent("pin-field", "The PIN typed during setup should still be held")
        touchTheKey()
        expectText("entry-title", contains: "github.com")
    }

    func testAfterSetupANewEntrySavesWithoutTheKeyAndWaitsToBeSent() {
        launch()
        setUpFully()
        tap("folder-web")
        tap("add-button")
        let name = expect("form-name")
        name.tap()
        name.typeText("after-setup.example")
        let password = element("form-password")
        password.tap()
        password.typeText("Typed-pass-42")
        tap("form-save")
        expectGone("form-save")
        expect("entry-after-setup.example")
        waitForKeyTasksToEnd()
        expectAbsent("key-sheet-purpose", "Saving should never ask for the key")
        goBack()
        expectText("unpushed-banner", contains: "1 change not sent")
        snapshot("home-after-save")
    }

    func testSetupScreensInDark() {
        launch(appearance: "dark")
        setUpFully(shots: true)
    }

    // MARK: Relaunching

    func testARelaunchAfterSetupGoesStraightToHome() {
        launch()
        setUpFully()
        app.terminate()
        launch(keep: true)
        expect("home-title")
        expect("folder-web")
        expectAbsent("setup-welcome", "A set-up store should not show setup again")
    }

    func testARelaunchAfterTheDownloadResumesAtTheKeyCheck() {
        launch()
        toAddress()
        toDownload()
        download()
        app.terminate()
        launch(keep: true)
        expect("setup-check")
        expectText("check-count", contains: "29 passwords")
        // A new launch holds no PIN, so the check asks for it, then a touch.
        tap("check-start")
        enterPin(Self.pin)
        touchTheKey()
        expect("setup-done")
    }

    // MARK: The address

    func testTheAddressLineSaysWherePawWillConnectAndALongAddressWraps() {
        launch()
        toAddress()
        let field = expect("repo-address")
        let oneLine = field.frame.height
        // A long address wraps in the field instead of being cut off.
        let long = "ssh://git@a-long-host-name.internal.example.com:2222/infrastructure-team/shared/password-store.git"
        field.tap()
        field.typeText(long)
        expectValue("repo-address", long)
        expectText("address-reading", contains: "Connects to a-long-host-name.internal.example.com as git on port 2222.")
        XCTAssertGreaterThan(field.frame.height, oneLine * 1.5, "The long address should take more than one line")
        snapshot("address-long")
        // GitHub's short form, read as its SSH address, with a host whose
        // keys come with paw.
        clearAddress()
        field.typeText("you/password-store")
        expectText("address-reading", contains: "Connects to github.com as git.")
        expectText("address-reading", contains: "Its fingerprint is built in.")
        expectText("address-reading-notice", contains: "git@github.com:you/password-store.git")
        snapshot("address-short-form")
        // The simulated host: its own port, a fingerprint to confirm.
        tap("simulated-address")
        expectText("address-reading", contains: "Connects to 127.0.0.1 as git on port")
        expectAbsent("address-reading-notice", "The simulated host's address is used as typed")
        snapshot("address-reading")
    }

    // MARK: What goes wrong

    func testABadAddressShowsTheProblemInlineAndStays() {
        launch()
        toAddress()
        let field = expect("repo-address")
        field.tap()
        field.typeText("password-store")
        tap("address-connect")
        expectText("address-problem", contains: "doesn't look like a git address")
        expect("setup-address")
        expectAbsent("setup-confirm-host", "A bad address should go nowhere")
        snapshot("bad-address")
    }

    func testAKeyWithAPinSettingPawCannotFollowIsRefusedBeforeAnyPin() {
        launch(extra: ["-pawSimKdf", "unsupported"])
        expect("setup-welcome")
        tap("setup-start")
        expectText("error-row", contains: "paw doesn't support this key's PIN setting yet")
        expectText("error-row", contains: "No PIN try was used")
        waitForKeyTasksToEnd()
        expect("setup-plug-in")
        expectAbsent("pin-field", "No PIN is asked of a key paw can't send it to")
        expectAbsent("setup-your-key", "Setup stops at the key")
        snapshot("kdf-unsupported")
    }

    func testAKeyThatFailsToBeReadShowsWhyAndTryAgainReadsIt() {
        // Dark here: a cheap screen to see in both.
        // The key is read at launch and again on Set up: both fail.
        launch(appearance: "dark", extra: ["-pawSimAttachFails", "2"])
        expect("setup-welcome")
        tap("setup-start")
        expectText("error-row", contains: "paw couldn't read your key")
        expectText("error-row", contains: "didn't answer")
        expect("setup-plug-in")
        snapshot("attach-failed")
        // No unplugging: Try again reads the key that is still in.
        tap("plug-in-retry")
        expect("setup-your-key")
        expectText("key-serial", contains: "serial")
        expectGone("plug-in-retry")
    }

    func testAHostThatDoesNotKnowTheKeyShowsTheKeyToAddBeforeAnyPin() {
        launch(remote: "refuseKey")
        toAddress()
        tap("simulated-address")
        tap("address-connect")
        tap("host-trust")
        expect("setup-key-not-accepted")
        expectText("ssh-public-key", contains: "ssh-ed25519")
        expect("copy-ssh-key")
        waitForKeyTasksToEnd()
        expectAbsent("pin-field", "No PIN is asked before the host accepts the key")
        snapshot("key-not-accepted")
        tap("copy-ssh-key")
        tap("simulated-accept-key")
        tap("check-again")
        expect("setup-download")
    }

    func testARepositoryTheHostDoesNotHaveSaysNoStoreAtThatAddress() {
        launch(remote: "repositoryNotFound")
        toAddress()
        toDownload()
        // The host says so only after the key's login: PIN and touch.
        tap("download-start")
        enterPin(Self.pin)
        touchTheKey()
        expectText("error-row", contains: "No store at that address", timeout: 60)
        expectText("error-row", contains: "store.git doesn't exist, or this key can't see it")
        waitForKeyTasksToEnd()
        expect("setup-download")
        expectAbsent("setup-check", "Nothing was downloaded")
        snapshot("repository-not-found")
    }

    func testAChangedHostKeyIsRefusedAndAnExpectedChangeIsConfirmedAgain() {
        launch(remote: "changedHostKey")
        toAddress()
        tap("simulated-address")
        tap("address-connect")
        let first = expect("host-fingerprint").label
        tap("host-trust")
        expect("setup-download")
        // The host now presents another key.
        tap("download-start")
        enterPin(Self.pin)
        expectText("error-row", contains: "identity changed")
        expectText("error-row", contains: "Nothing was sent")
        waitForKeyTasksToEnd()
        expectAbsent("touch-prompt", "Nothing is signed for a host whose identity changed")
        expect("setup-download")
        expect("host-key-expected")
        snapshot("host-key-changed")
        // Not a bundled host: the person may say they expected it, and
        // confirm the new key as on first use.
        tap("host-key-expected")
        expect("setup-confirm-host")
        let fingerprint = expect("host-fingerprint")
        let changed = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "label != %@", first), object: fingerprint)
        XCTAssertEqual(XCTWaiter().wait(for: [changed], timeout: Self.patience), .completed,
                       "The confirm screen should show the new fingerprint")
        snapshot("confirm-new-host-key")
        tap("host-trust")
        expect("setup-download")
        // The PIN was verified before the refusal, so it is still held.
        tap("download-start")
        expect("touch-prompt")
        expectAbsent("pin-field", "The PIN should still be held")
        tapSimulated("simulated-touch")
        expect("setup-check", timeout: 60)
    }

    func testADroppedConnectionOffersARetryThatSucceeds() {
        launch(remote: "dropMidClone")
        toAddress()
        toDownload()
        tap("download-start")
        enterPin(Self.pin)
        touchTheKey()
        expectText("error-row", contains: "Nothing was kept", timeout: 60)
        expect("setup-download")
        snapshot("dropped")
        tap("error-action")
        // The PIN is still held: straight to the touch.
        expect("touch-prompt")
        expectAbsent("pin-field", "The PIN should still be held for the retry")
        tapSimulated("simulated-touch")
        expect("setup-check", timeout: 60)
    }

    func testATouchTimeoutDuringTheDownloadSaysNothingWasDownloadedAndRetriesWithoutThePin() {
        launch()
        toAddress()
        toDownload()
        tap("download-start")
        enterPin(Self.pin)
        timeOutTheTouch()
        expectText("key-sheet-headline", contains: "Your key wasn't touched in time")
        expectText("key-sheet-body", contains: "touch the gold part while it blinks")
        snapshot("download-touch-timed-out")
        tap("key-sheet-retry")
        expect("touch-prompt")
        expectAbsent("pin-field", "The PIN should still be held after a touch timeout")
        tapSimulated("simulated-touch")
        expect("setup-check", timeout: 60)
    }

    func testAWrongPinDuringTheDownloadShowsTriesLeftAndStays() {
        launch()
        toAddress()
        toDownload()
        tap("download-start")
        enterPin("000000")
        expectText("pin-tries-left", contains: "2 tries left")
        expectText("key-sheet-headline", contains: "Wrong PIN")
        expect("pin-field")
        expectAbsent("touch-prompt", "A wrong PIN never reaches the touch")
        snapshot("wrong-pin")
    }

    func testCancelDuringTheDownloadLeavesNothingHalfDownloaded() {
        launch()
        toAddress()
        toDownload()
        tap("download-start")
        enterPin(Self.pin)
        expect("touch-prompt")
        tap("key-sheet-cancel")
        expectGone("key-sheet-cancel")
        // The cancelled login's command still waits on the key for its
        // touch, which nothing can interrupt: its timeout ends it.
        tap("simulated-time-out-touch")
        expectGone("simulated-time-out-touch")
        waitForKeyTasksToEnd()
        expect("download-start")
        expectAbsent("home-title", "A cancelled download is not a store")
        expectAbsent("setup-check", "A cancelled download is not a store")
        snapshot("cancelled")
        // Again: the PIN is held, and the key is free.
        tap("download-start")
        expect("touch-prompt")
        expectAbsent("pin-field", "The PIN should still be held after a cancel")
        tapSimulated("simulated-touch")
        expect("setup-check", timeout: 60)
    }

    // MARK: Settings

    func testRemovingTheStoreReturnsToWelcome() {
        launch()
        setUpFully()
        openSettings()
        // The simulated host is not bundled: the person trusted it in setup.
        scrollTo("settings-host-fingerprint")
        expectText("settings-host-fingerprint", contains: "Trusted by you")
        tap("settings-host-fingerprint")
        _ = scrollTo("settings-host-fingerprints")
        snapshot("settings-server")
        tap("settings-remove-store")
        // The confirmation opens at the foot of the list, below the fold.
        scrollTo("settings-remove-confirm")
        scrollTo("settings-remove-warning")
        expectText("settings-remove-warning", contains: "not touched")
        snapshot("remove-confirm")
        tap("settings-remove-confirm")
        expect("setup-welcome")
        app.terminate()
        launch(keep: true)
        expect("setup-welcome")
    }
}
