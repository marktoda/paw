import XCTest

/// Sending and checking for changes on the fixture store (`-pawFixture 1`),
/// which syncs with the core's simulated git host. `-pawSimRemoteChange`
/// has another computer change the host before paw fetches; the sync
/// sheet's simulator section changes it by hand. The hidden element
/// `sim-remote-summary` says the host's tip and how many logins it saw.
final class SyncTests: XCTestCase {
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

    private func launch(appearance: String = "light", extra: [String] = []) {
        app.launchArguments = [
            "-pawFixture", "1",
            "-pawAppearance", appearance,
            "-pawSimPin", Self.pin,
            "-pawSimTouch", "1",
            "-pawSimTouchTimeoutMs", "15000",
        ] + extra
        app.launch()
        expect("home-title", timeout: 30)
        expect("sync-pill", timeout: 30)
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
    private func expect(_ identifier: String, timeout: TimeInterval = 30,
                        file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let found = element(identifier)
        XCTAssertTrue(found.waitForExistence(timeout: timeout),
                      "\"\(identifier)\" should be on screen", file: file, line: line)
        return found
    }

    private func expectGone(_ identifier: String, timeout: TimeInterval = 30,
                            file: StaticString = #filePath, line: UInt = #line) {
        let gone = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "exists == false"), object: element(identifier))
        if XCTWaiter().wait(for: [gone], timeout: timeout) != .completed {
            XCTFail("\"\(identifier)\" should have gone", file: file, line: line)
        }
    }

    private func expectLabel(_ identifier: String, contains text: String, timeout: TimeInterval = 30,
                             file: StaticString = #filePath, line: UInt = #line) {
        let found = expect(identifier, timeout: timeout, file: file, line: line)
        let matches = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "label CONTAINS %@", text), object: found)
        if XCTWaiter().wait(for: [matches], timeout: timeout) != .completed {
            XCTFail("\"\(identifier)\" should say \"\(text)\", it said \"\(found.label)\"",
                    file: file, line: line)
        }
    }

    /// Scrolls until the element can be tapped.
    @discardableResult
    private func scrollTo(_ identifier: String, file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let found = element(identifier)
        _ = found.waitForExistence(timeout: 20)
        var swipes = 0
        while !(found.exists && found.isHittable && !underKeyboard(found)) && swipes < 6 {
            app.swipeUp()
            swipes += 1
        }
        XCTAssertTrue(found.exists, "\"\(identifier)\" should be on screen", file: file, line: line)
        return found
    }

    private func underKeyboard(_ found: XCUIElement) -> Bool {
        let keyboard = app.keyboards.firstMatch
        return keyboard.exists && found.frame.maxY > keyboard.frame.minY
    }

    private func tap(_ identifier: String, file: StaticString = #filePath, line: UInt = #line) {
        scrollTo(identifier, file: file, line: line).tap()
    }

    /// The action button of the inline row `container` (an `ErrorRow`).
    private func tapAction(in container: String, file: StaticString = #filePath, line: UInt = #line) {
        let button = expect(container, file: file, line: line).buttons["error-action"].firstMatch
        XCTAssertTrue(button.waitForExistence(timeout: 20), "\"\(container)\" should offer its action",
                      file: file, line: line)
        button.tap()
    }

    private func goBack() {
        let back = app.navigationBars.buttons["BackButton"]
        if back.exists {
            back.tap()
        } else {
            app.navigationBars.buttons.element(boundBy: 0).tap()
        }
    }

    private func type(_ text: String, into identifier: String) {
        let field = scrollTo(identifier)
        field.tap()
        let current = (field.value as? String) ?? ""
        let placeholder = field.placeholderValue ?? ""
        if !current.isEmpty && current != placeholder {
            field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: current.count))
        }
        if !text.isEmpty {
            field.typeText(text)
        }
    }

    /// From Home: a new entry with a typed password, saved. No key.
    private func addEntry(_ name: String, folder: String) {
        tap("add-button")
        expect("form-name")
        type(name, into: "form-name")
        if !folder.isEmpty {
            tap("form-folder")
            let item = app.buttons[folder].firstMatch
            XCTAssertTrue(item.waitForExistence(timeout: 20), "The folder menu should list \(folder)")
            item.tap()
        }
        type("Typed-pass-42", into: "form-password")
        tap("form-save")
        expectGone("form-save")
        expectLabel("sim-last-app-toast", contains: "Saved. Not sent yet.")
    }

    /// The tip and logins of the simulated host, as the debug element says.
    private var remote: String {
        let found = element("sim-remote-summary")
        _ = found.waitForExistence(timeout: 20)
        return found.label
    }

    /// The key sheet for a sync is up: types the PIN if asked for, touches
    /// the key if it waits for a touch, and waits for the sheet to finish.
    /// `pin == false` fails if the PIN is asked for.
    private func passKeySheet(purpose: String, pin: Bool, file: StaticString = #filePath, line: UInt = #line) {
        expectLabel("key-sheet-purpose", contains: purpose, file: file, line: line)
        if pin {
            let field = expect("pin-field", file: file, line: line)
            field.tap()
            field.typeText(Self.pin + "\n")
            expectGone("pin-field", file: file, line: line)
        }
        touchIfAsked(file: file, line: line)
    }

    /// Waits for the sheet to touch or to finish. A touch is asked for
    /// only when the key's touch is not still cached.
    private func touchIfAsked(file: StaticString = #filePath, line: UInt = #line) {
        let touch = element("touch-prompt")
        let purpose = element("key-sheet-purpose")
        let pinField = element("pin-field")
        let deadline = Date().addingTimeInterval(45)
        while Date() < deadline {
            if touch.exists {
                app.buttons["simulated-touch"].firstMatch.tap()
                expectGone("touch-prompt", file: file, line: line)
                return
            }
            if !purpose.exists {
                return
            }
            if pinField.exists {
                XCTFail("The PIN should not be asked for again", file: file, line: line)
                return
            }
            _ = touch.waitForExistence(timeout: 0.5)
        }
        XCTFail("The key sheet neither asked for a touch nor finished", file: file, line: line)
    }

    /// Home's pill, then the sync sheet.
    private func openSyncSheet() {
        tap("sync-pill")
        expect("sync-sheet-done")
    }

    // MARK: Sending

    func testSendingAsksForThePinOnceEmptiesTheListAndMovesTheRemoteAndASecondSendAsksForNoPin() {
        launch()
        expectLabel("sync-pill", contains: "Updates need your key")
        snapshot("pill-needs-key-light")
        let before = remote

        addEntry("sent.example", folder: "web")
        expectLabel("unpushed-banner", contains: "1 change not sent")
        snapshot("pill-not-sent-light")
        tap("unpushed-banner")
        expect("unpushed-send")
        snapshot("unpushed-send-light")
        tap("unpushed-send")
        passKeySheet(purpose: "Send 1 change", pin: true)
        expectGone("key-sheet-purpose")
        expectLabel("sim-last-app-toast", contains: "Sent")
        expectGone("unpushed-count")
        expectGone("unpushed-send")
        snapshot("unpushed-after-send-light")
        goBack()
        expectLabel("sync-pill", contains: "Up to date")
        XCTAssertNotEqual(remote, before, "The simulated host's tip should have moved")
        snapshot("pill-up-to-date-light")

        // A second send in the same run: the PIN is still held.
        addEntry("again.example", folder: "web")
        openSyncSheet()
        expect("sync-send")
        snapshot("sync-sheet-light")
        let middle = remote
        tap("sync-send")
        passKeySheet(purpose: "Send 1 change", pin: false)
        expectGone("key-sheet-purpose")
        expectLabel("sync-pill", contains: "Up to date")
        XCTAssertNotEqual(remote, middle, "The second send should have moved the tip again")
        XCTAssertFalse(element("unpushed-banner").exists, "Nothing should be waiting")
    }

    func testThePillAndTheSyncSheetInDark() {
        launch(appearance: "dark")
        expectLabel("sync-pill", contains: "Updates need your key")
        snapshot("pill-needs-key-dark")
        addEntry("dark.example", folder: "")
        expectLabel("unpushed-banner", contains: "1 change not sent")
        snapshot("pill-not-sent-dark")
        openSyncSheet()
        snapshot("sync-sheet-not-sent-dark")
        tap("sync-send")
        passKeySheet(purpose: "Send 1 change", pin: true)
        expectGone("key-sheet-purpose")
        expectLabel("sync-pill", contains: "Up to date")
        snapshot("pill-up-to-date-dark")
        openSyncSheet()
        expectLabel("sync-last-checked", contains: "Just now")
        snapshot("sync-sheet-up-to-date-dark")
    }

    func testSendingAfterTheRemoteMovedStillSucceedsOnTopOfIt() {
        launch(extra: ["-pawSimRemoteChange", "add"])
        let before = remote
        addEntry("moved.example", folder: "web")
        openSyncSheet()
        tap("sync-send")
        passKeySheet(purpose: "Send 1 change", pin: true)
        expectGone("key-sheet-purpose")
        expectLabel("sim-last-app-toast", contains: "Sent")
        expectLabel("sync-pill", contains: "Up to date")
        XCTAssertFalse(element("unpushed-banner").exists, "Nothing should be waiting")
        XCTAssertNotEqual(remote, before)
        // What the other computer added came in on the way.
        expect("sync-added-forum.example")
        snapshot("moved-summary")
    }

    func testCancellingASendLeavesTheQueueIntact() {
        launch()
        let before = remote
        addEntry("cancel.example", folder: "web")
        tap("unpushed-banner")
        tap("unpushed-send")
        expectLabel("key-sheet-purpose", contains: "Send 1 change")
        let field = expect("pin-field")
        field.tap()
        field.typeText(Self.pin + "\n")
        expect("touch-prompt")
        tap("key-sheet-cancel")
        expectGone("key-sheet-purpose")
        expectLabel("unpushed-count", contains: "1 change")
        expect("unpushed-send")
        goBack()
        expectLabel("unpushed-banner", contains: "1 change not sent")
        XCTAssertEqual(remote, before, "Nothing should have been sent")
        openSyncSheet()
        expectLabel("sync-cancelled", contains: "Sending was cancelled.")
        snapshot("cancelled")
    }

    func testRefusedPushesAreWordedAndTheChangesStayQueued() {
        launch(extra: ["-pawSimRemoteRefusePushes", "1"])
        let before = remote
        addEntry("refused.example", folder: "web")
        tap("unpushed-banner")
        tap("unpushed-send")
        passKeySheet(purpose: "Send 1 change", pin: true)
        expect("sync-failure")
        expectLabel("error-row", contains: "refused your changes")
        expectLabel("error-row", contains: "Your change is saved on this iPhone.")
        snapshot("push-refused")
        tap("sync-sheet-done")
        expectGone("sync-sheet-done")
        expectLabel("unpushed-count", contains: "1 change")
        XCTAssertEqual(remote.components(separatedBy: " · ").first, before.components(separatedBy: " · ").first,
                       "The tip should not have moved")
        goBack()
        expectLabel("sync-pill", contains: "Not sent")
    }

    func testADroppedConnectionOffersARetryThatSucceeds() {
        launch(extra: ["-pawSimRemoteChange", "add", "-pawSimRemoteDrop", "1"])
        tap("sync-pill")
        passKeySheet(purpose: "Update your store", pin: true)
        expect("sync-failure")
        snapshot("dropped")
        tapAction(in: "sync-failure")
        touchIfAsked()
        expectGone("key-sheet-purpose")
        expect("sync-added-forum.example")
        expectLabel("sync-pill", contains: "Up to date")
    }

    // MARK: Checking for changes

    func testCheckingForChangesNamesARemoteAdditionThatOpensWithTheKey() {
        launch(extra: ["-pawSimRemoteChange", "add"])
        tap("sync-pill")
        passKeySheet(purpose: "Update your store", pin: true)
        expectGone("key-sheet-purpose")
        expectLabel("sync-summary-title", contains: "1 password changed on your other devices")
        expect("sync-added-forum.example")
        snapshot("summary-added")
        tap("sync-added-forum.example")
        passKeySheet(purpose: "Open forum.example", pin: false)
        expectLabel("entry-title", contains: "forum.example")
        goBack()
        tap("sync-summary-dismiss")
        expectGone("sync-summary-title")
    }

    func testARemoteEditEndsTheFaceIDCopyOfThatEntry() {
        launch(extra: ["-pawSimRemoteChange", "edit"])
        tap("folder-web")
        tap("entry-news.ycombinator.com")
        passKeySheet(purpose: "Open news.ycombinator.com", pin: true)
        expectLabel("entry-title", contains: "news.ycombinator.com")
        goBack()
        goBack()
        expect("face-id-news.ycombinator.com")
        tap("sync-pill")
        passKeySheet(purpose: "Update your store", pin: false)
        expectGone("key-sheet-purpose")
        expect("sync-changed-news.ycombinator.com")
        expectGone("face-id-news.ycombinator.com")
        expectLabel("sync-summary-ended-copies", contains: "news.ycombinator.com")
        snapshot("face-id-copy-ended")
    }

    func testBothSidesEditingOneEntryKeepsBothAndListsThem() {
        launch(extra: ["-pawSimRemoteChange", "conflict"])
        tap("folder-web")
        tap("entry-example.com")
        passKeySheet(purpose: "Open example.com", pin: true)
        expectLabel("entry-title", contains: "example.com")
        tap("entry-edit")
        type("Edited-here-7", into: "form-password")
        tap("form-save")
        expectGone("form-save")
        goBack()
        goBack()
        openSyncSheet()
        tap("sync-send")
        passKeySheet(purpose: "Send 1 change", pin: false)
        expectGone("key-sheet-purpose")
        expectLabel("sync-summary-title", contains: "Two versions of example.com were kept")
        expectLabel("sync-kept-both-example.com", contains: "Both versions were kept")
        expect("sync-kept-both-remote-example.com")
        expect("sync-kept-both-local-example.com")
        snapshot("kept-both")
        tap("folder-web")
        expect("entry-example.com")
        let copies = app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier BEGINSWITH %@", "entry-example.com (from this iPhone"))
        XCTAssertTrue(copies.firstMatch.waitForExistence(timeout: 20), "This iPhone's version should be listed beside it")
    }

    // MARK: What a sync can't do by itself

    func testAChangedGpgIdHoldsBackTheQueuedChangeAndSendsNothing() {
        launch(extra: ["-pawSimRemoteChange", "gpgid"])
        let before = remote
        addEntry("held.example", folder: "work")
        openSyncSheet()
        tap("sync-send")
        passKeySheet(purpose: "Send 1 change", pin: true)
        expect("sync-held-back")
        expectAbsent("sync-send", "A held-back change is never offered to send as it is")
        snapshot("held-back-sheet")
        tapAction(in: "sync-held-back")
        expect("held-back-held.example")
        expectLabel("held-back-should-be-for-held.example", contains: "Not accepted on this iPhone yet.")
        snapshot("held-back-screen")
        XCTAssertEqual(remote.components(separatedBy: " · ").first, before.components(separatedBy: " · ").first,
                       "Nothing should have been sent")

        // Accepting the new list (the same key here) lets it go.
        tap("held-back-review-held.example")
        tap("recipients-accept")
        expect("held-back-empty")
    }

    func testARefusedRemoteIsExplainedAndTheQueueIsKept() {
        launch(extra: ["-pawSimRemoteChange", "link"])
        addEntry("kept.example", folder: "web")
        tap("unpushed-banner")
        tap("unpushed-send")
        passKeySheet(purpose: "Send 1 change", pin: true)
        expect("sync-refused-remote")
        snapshot("refused-remote-sheet")
        tapAction(in: "sync-refused-remote")
        expectLabel("refused-path", contains: "Web")
        snapshot("refused-remote-screen")
        goBack()
        expectLabel("unpushed-count", contains: "1 change")
        goBack()
        expectLabel("sync-pill", contains: "Sync paused")
    }

    func testReplacedHistoryIsExplainedAndKeepsTheChangesWhenAsked() {
        replaceHistoryWithAChangeWaiting()
        snapshot("replaced-screen")
        tap("replaced-not-now")
        expectGone("replaced-intro")
        expectLabel("sync-pill", contains: "Sync paused")
        snapshot("replaced-paused")
        openSyncSheet()
        tapAction(in: "sync-replaced")
        expect("replaced-intro")
        tap("replaced-keep")
        expectGone("replaced-intro")
        expectLabel("unpushed-banner", contains: "1 change not sent")
        expect("sync-added-new-history-1")

        let before = remote
        openSyncSheet()
        tap("sync-send")
        passKeySheet(purpose: "Send 1 change", pin: false)
        expectGone("key-sheet-purpose")
        expectLabel("sync-pill", contains: "Up to date")
        XCTAssertNotEqual(remote, before)
    }

    func testReplacedHistoryDiscardsTheChangesOnlyAfterAConfirm() {
        replaceHistoryWithAChangeWaiting()
        tap("replaced-discard-button")
        expectLabel("replaced-discard-question", contains: "Discard 1 change?")
        snapshot("replaced-discard-confirm")
        tap("replaced-discard-cancel")
        expect("replaced-discard-button")
        tap("replaced-discard-button")
        tap("replaced-discard-confirm")
        expectGone("replaced-intro")
        expectGone("unpushed-banner")
        expectLabel("sync-pill", contains: "Up to date")
    }

    /// A change waiting, the history replaced on the host, then a send:
    /// paw stops and the careful screen is up.
    private func replaceHistoryWithAChangeWaiting() {
        launch(extra: ["-pawSimRemoteChange", "rewrite"])
        addEntry("waiting.example", folder: "web")
        openSyncSheet()
        tap("sim-controls")
        tap("sim-remote-rewrite")
        expectLabel("sim-remote-status", contains: "Replaced the history.")
        tap("sync-sheet-done")
        expectGone("sync-sheet-done")
        openSyncSheet()
        tap("sync-send")
        passKeySheet(purpose: "Send 1 change", pin: true)
        expect("sync-replaced")
        expectAbsent("sync-send", "Nothing is sent onto a replaced history before the person chooses")
        snapshot("replaced-sheet")
        tapAction(in: "sync-replaced")
        expect("replaced-intro")
        expectLabel("replaced-unpushed", contains: "Your 1 change not sent yet")
    }

    private func expectAbsent(_ identifier: String, _ why: String,
                              file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertFalse(element(identifier).exists, why, file: file, line: line)
    }
}
