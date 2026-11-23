import XCTest

/// First-run setup (`-pawSetup 1`) against the simulated key (PIN 123456,
/// a touch for every use) and the core's simulated git host, which runs
/// inside the app and serves the fixture store: 29 passwords, 27 of them
/// for the simulated key.
final class SetupTests: XCTestCase {
    private var app: XCUIApplication!

    private static let pin = "123456"

    override func setUp() {
        continueAfterFailure = false
        app = XCUIApplication()
    }

    override func tearDown() {
        snapshot("end")
    }

    // MARK: Helpers

    /// `keep` keeps what earlier launches set up; otherwise setup starts
    /// from nothing.
    private func launch(appearance: String = "light", remote: String? = nil, keep: Bool = false,
                        touchTimeoutMs: Int = 15_000) {
        var arguments = [
            "-pawSetup", "1",
            "-pawAppearance", appearance,
            "-pawSimPin", Self.pin,
            "-pawSimTouch", "1",
            "-pawSimTouchTimeoutMs", String(touchTimeoutMs),
        ]
        if keep { arguments += ["-pawSetupKeep", "1"] }
        if let remote { arguments += ["-pawSimRemote", remote] }
        app.launchArguments = arguments
        app.launch()
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

    private func expectAbsent(_ identifier: String, _ why: String,
                              file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertFalse(element(identifier).exists, why, file: file, line: line)
    }

    /// Waits until the element's label (or value) contains `text`.
    private func expectText(_ identifier: String, contains text: String, timeout: TimeInterval = 10,
                            file: StaticString = #filePath, line: UInt = #line) {
        let found = expect(identifier, timeout: timeout, file: file, line: line)
        let matches = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "label CONTAINS %@ OR value CONTAINS %@", text, text), object: found)
        if XCTWaiter().wait(for: [matches], timeout: timeout) != .completed {
            XCTFail("\"\(identifier)\" should say \"\(text)\", it said \"\(found.label)\" \(String(describing: found.value))",
                    file: file, line: line)
        }
    }

    private func tap(_ identifier: String, timeout: TimeInterval = 10,
                     file: StaticString = #filePath, line: UInt = #line) {
        expect(identifier, timeout: timeout, file: file, line: line).tap()
    }

    private func enterPin(_ pin: String, file: StaticString = #filePath, line: UInt = #line) {
        let field = expect("pin-field", file: file, line: line)
        field.tap()
        field.typeText(pin)
        app.buttons["pin-continue"].tap()
    }

    /// The touch prompt is up: the simulated finger goes on the key at once.
    private func touchKey(file: StaticString = #filePath, line: UInt = #line) {
        expect("touch-prompt", timeout: 20, file: file, line: line)
        app.buttons["simulated-touch"].tap()
    }

    /// Welcome, "Your key", then the address screen.
    private func toAddress(shots: Bool = false) {
        expect("setup-welcome", timeout: 20)
        if shots { snapshot("welcome") }
        tap("setup-start")
        expect("setup-your-key", timeout: 15)
        expectText("key-serial", contains: "serial")
        if shots { snapshot("your-key") }
        tap("key-continue")
        expect("repo-address")
    }

    /// Types the simulated host's address and trusts its key.
    private func toDownload(shots: Bool = false) {
        tap("simulated-address")
        if shots { snapshot("address") }
        tap("address-connect")
        expect("setup-confirm-host", timeout: 20)
        expect("host-fingerprint")
        if shots { snapshot("confirm-host") }
        tap("host-trust")
        expect("setup-download", timeout: 20)
        if shots { snapshot("download") }
    }

    /// Download: the PIN once (unless held), a touch for the login.
    private func download(pin: Bool = true, shots: Bool = false) {
        tap("download-start")
        if pin {
            enterPin(Self.pin)
        }
        expect("touch-prompt", timeout: 20)
        if shots { snapshot("download-touch") }
        app.buttons["simulated-touch"].tap()
        if shots {
            // What follows the touch: the clone with its progress, or Done
            // if the fixture's 29 entries have already arrived.
            snapshot("download-progress")
        }
        expect("setup-check", timeout: 60)
    }

    /// The key check: no PIN (it is held), one touch.
    private func check(shots: Bool = false) {
        if shots { snapshot("check") }
        tap("check-start")
        expect("touch-prompt", timeout: 20)
        expectAbsent("pin-field", "The PIN typed for the download should still be held")
        touchKey()
        expect("setup-done", timeout: 30)
    }

    private func setUpFully(shots: Bool = false) {
        toAddress(shots: shots)
        toDownload(shots: shots)
        download(shots: shots)
        check(shots: shots)
        expectText("done-count", contains: "29 passwords")
        if shots { snapshot("done") }
        tap("done-open")
        expect("home-title", timeout: 20)
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
        expect("touch-prompt", timeout: 20)
        expectAbsent("pin-field", "The PIN typed during setup should still be held")
        touchKey()
        expectText("entry-title", contains: "github.com", timeout: 20)
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
        expect("home-title", timeout: 20)
        expectAbsent("setup-welcome", "A set-up store should not show setup again")
        expect("folder-web")
    }

    func testARelaunchAfterTheDownloadResumesAtTheKeyCheck() {
        launch()
        toAddress()
        toDownload()
        download()
        app.terminate()
        launch(keep: true)
        expect("setup-check", timeout: 20)
        expectText("check-count", contains: "29 passwords")
        // A new launch holds no PIN, so the check asks for it, then a touch.
        tap("check-start")
        enterPin(Self.pin)
        touchKey()
        expect("setup-done", timeout: 30)
    }

    // MARK: What goes wrong

    func testABadAddressShowsTheProblemInlineAndStays() {
        launch()
        toAddress()
        let field = expect("repo-address")
        field.tap()
        field.typeText("you/password-store")
        tap("address-connect")
        expectText("address-problem", contains: "doesn't look like a git address")
        expect("setup-address")
        expectAbsent("setup-confirm-host", "A bad address should go nowhere")
        snapshot("bad-address")
    }

    func testAHostThatDoesNotKnowTheKeyShowsTheKeyToAddBeforeAnyPin() {
        launch(remote: "refuseKey")
        toAddress()
        tap("simulated-address")
        tap("address-connect")
        tap("host-trust", timeout: 20)
        expect("setup-key-not-accepted", timeout: 20)
        expectText("ssh-public-key", contains: "ssh-ed25519")
        expect("copy-ssh-key")
        expectAbsent("pin-field", "No PIN is asked before the host accepts the key")
        snapshot("key-not-accepted")
        tap("copy-ssh-key")
        tap("simulated-accept-key")
        tap("check-again")
        expect("setup-download", timeout: 20)
    }

    func testAChangedHostKeyIsRefusedWithTheExplanation() {
        launch(remote: "changedHostKey")
        toAddress()
        toDownload()
        tap("download-start")
        enterPin(Self.pin)
        expectText("error-row", contains: "identity changed", timeout: 30)
        expectText("error-row", contains: "Nothing was sent")
        expectAbsent("touch-prompt", "Nothing is signed for a host whose identity changed")
        expect("setup-download")
        snapshot("host-key-changed")
    }

    func testADroppedConnectionOffersARetryThatSucceeds() {
        launch(remote: "dropMidClone")
        toAddress()
        toDownload()
        tap("download-start")
        enterPin(Self.pin)
        touchKey()
        expectText("error-row", contains: "Nothing was kept", timeout: 60)
        expect("setup-download")
        snapshot("dropped")
        tap("error-action")
        // The PIN is still held: straight to the touch.
        expect("touch-prompt", timeout: 20)
        expectAbsent("pin-field", "The PIN should still be held for the retry")
        touchKey()
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
        launch(touchTimeoutMs: 4_000)
        toAddress()
        toDownload()
        tap("download-start")
        enterPin(Self.pin)
        expect("touch-prompt", timeout: 20)
        tap("key-sheet-cancel")
        expect("download-start", timeout: 10)
        expectAbsent("home-title", "A cancelled download is not a store")
        expectAbsent("setup-check", "A cancelled download is not a store")
        snapshot("cancelled")
        // Again: the PIN is held; the key may first finish the cancelled login.
        tap("download-start")
        touchKey()
        expect("setup-check", timeout: 60)
    }

    // MARK: Settings

    func testRemovingTheStoreReturnsToWelcome() {
        launch()
        setUpFully()
        tap("settings-button")
        expect("settings-address")
        let remove = element("settings-remove-store")
        var swipes = 0
        while !(remove.exists && remove.isHittable) && swipes < 6 {
            app.swipeUp()
            swipes += 1
        }
        remove.tap()
        expectText("settings-remove-warning", contains: "not touched")
        snapshot("remove-confirm")
        tap("settings-remove-confirm")
        expect("setup-welcome", timeout: 20)
        app.terminate()
        launch(keep: true)
        expect("setup-welcome", timeout: 20)
    }
}
