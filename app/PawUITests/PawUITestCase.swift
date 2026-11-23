import XCTest

/// What every UI test class shares: launching, finding elements, waiting on
/// conditions, the key sheet and the simulated hardware.
///
/// Waits are on conditions, never on time. Nothing here reads a label once
/// and compares it: a label read mid-update is how a slow machine makes a
/// test fail that a fast one passes. Negative checks ("nothing opened")
/// come after `waitForKeyTasksToEnd()`, the app's own signal that every key
/// task it started has finished, so they cannot pass vacuously.
///
/// Waits poll every fraction of a second (`waitUntil`) rather than use
/// `waitForExistence` or `XCTWaiter`: those first look a full second after
/// they start, even when the element is already there, which was a fifth
/// of every test's time.
class PawUITestCase: XCTestCase {
    var app: XCUIApplication!

    /// Launches paw with UIKit's animations off (`-pawNoAnimations 1`), so
    /// a push, sheet or menu is finished as soon as it starts and nothing
    /// waits for it to settle. A test about an animation sets
    /// `realAnimations` before launching.
    final class TestApp: XCUIApplication {
        var realAnimations = false

        override func launch() {
            if !realAnimations && !launchArguments.contains("-pawNoAnimations") {
                launchArguments += ["-pawNoAnimations", "1"]
            }
            super.launch()
        }
    }

    static let pin = "123456"
    static let githubPassword = "tidal-Quartz-88-ferns"
    /// The simulated key's touch timeout in tests that are not about it:
    /// far longer than any slow machine needs to tap "touch". A test about
    /// the timeout ends the wait with `timeOutTheTouch()`.
    static let touchTimeoutMs = 120_000
    /// The longest a step may take on a slow CI machine.
    static let patience: TimeInterval = 30

    override func setUp() {
        continueAfterFailure = false
        app = TestApp()
    }

    override func tearDown() {
        // Every test leaves a picture of where it ended, pass or fail.
        snapshot("end")
    }

    // MARK: Launching

    /// The simulated key's arguments: PIN, a touch for every use, a long timeout.
    func keyArguments(touchTimeoutMs: Int = PawUITestCase.touchTimeoutMs) -> [String] {
        [
            "-pawSimPin", Self.pin,
            "-pawSimTouch", "1",
            "-pawSimTouchTimeoutMs", String(touchTimeoutMs),
        ]
    }

    /// The app on the fixture store (`-pawFixture 1`), at Home.
    func launchFixture(appearance: String = "light", extra: [String] = []) {
        app.launchArguments = ["-pawFixture", "1", "-pawAppearance", appearance] + keyArguments() + extra
        app.launch()
        expect("home-title", timeout: Self.patience)
    }

    // MARK: Waiting

    /// Checks `condition` now, then again every 0.05 s to 0.5 s until it
    /// holds or `timeout` has passed; it is always checked once more at the
    /// end. Each check of an element asks the app for its accessibility
    /// tree, which takes about half a second on a CI runner, so the checks
    /// themselves set the pace.
    @discardableResult
    func waitUntil(timeout: TimeInterval, _ condition: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        var pause: TimeInterval = 0.05
        while true {
            if condition() { return true }
            let now = Date()
            if now >= deadline { return false }
            RunLoop.current.run(until: min(deadline, now.addingTimeInterval(pause)))
            pause = min(pause * 2, 0.5)
        }
    }

    /// `waitForExistence` without its one-second first look.
    func waitToExist(_ found: XCUIElement, timeout: TimeInterval) -> Bool {
        waitUntil(timeout: timeout) { found.exists }
    }

    // MARK: Finding

    func snapshot(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = "\(self.name.split(separator: " ").last?.dropLast() ?? "test")-\(name)"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func element(_ identifier: String) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: identifier).firstMatch
    }

    @discardableResult
    func expect(_ identifier: String, timeout: TimeInterval = PawUITestCase.patience,
                file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let found = element(identifier)
        XCTAssertTrue(waitToExist(found, timeout: timeout),
                      "\"\(identifier)\" should be on screen", file: file, line: line)
        return found
    }

    func expectGone(_ identifier: String, timeout: TimeInterval = PawUITestCase.patience,
                    file: StaticString = #filePath, line: UInt = #line) {
        wait(for: "exists == false", on: element(identifier), timeout: timeout,
             "\"\(identifier)\" should have gone", file: file, line: line)
    }

    /// Not on screen now. Only meaningful once whatever could put it there
    /// has finished: see `waitForKeyTasksToEnd()`.
    func expectAbsent(_ identifier: String, _ why: String,
                      file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertFalse(element(identifier).exists, why, file: file, line: line)
    }

    /// Waits until the element's label contains `text`.
    func expectLabel(_ identifier: String, contains text: String, timeout: TimeInterval = PawUITestCase.patience,
                     file: StaticString = #filePath, line: UInt = #line) {
        let found = expect(identifier, timeout: timeout, file: file, line: line)
        let predicate = NSPredicate(format: "label CONTAINS %@", text)
        if !waitUntil(timeout: timeout, { predicate.evaluate(with: found) }) {
            XCTFail("\"\(identifier)\" should say \"\(text)\", it said \"\(found.label)\"", file: file, line: line)
        }
    }

    /// Waits until the element's label or value contains `text`.
    func expectText(_ identifier: String, contains text: String, timeout: TimeInterval = PawUITestCase.patience,
                    file: StaticString = #filePath, line: UInt = #line) {
        let found = expect(identifier, timeout: timeout, file: file, line: line)
        let predicate = NSPredicate(format: "label CONTAINS %@ OR value CONTAINS %@", text, text)
        if !waitUntil(timeout: timeout, { predicate.evaluate(with: found) }) {
            XCTFail("\"\(identifier)\" should say \"\(text)\", it said \"\(found.label)\" \(String(describing: found.value))",
                    file: file, line: line)
        }
    }

    /// Waits until the element's value is exactly `value`.
    func expectValue(_ identifier: String, _ value: String, timeout: TimeInterval = PawUITestCase.patience,
                     file: StaticString = #filePath, line: UInt = #line) {
        let found = expect(identifier, timeout: timeout, file: file, line: line)
        let predicate = NSPredicate(format: "value == %@", value)
        if !waitUntil(timeout: timeout, { predicate.evaluate(with: found) }) {
            XCTFail("\"\(identifier)\" should be \"\(value)\", it was \"\(found.value ?? "nil")\"",
                    file: file, line: line)
        }
    }

    func expectEnabled(_ identifier: String, _ enabled: Bool,
                       file: StaticString = #filePath, line: UInt = #line) {
        let found = expect(identifier, file: file, line: line)
        wait(for: "enabled == \(enabled ? "true" : "false")", on: found, timeout: Self.patience,
             "\"\(identifier)\" should be \(enabled ? "enabled" : "disabled")", file: file, line: line)
    }

    func wait(for predicate: String, on object: XCUIElement, timeout: TimeInterval, _ message: String,
              file: StaticString = #filePath, line: UInt = #line) {
        let condition = NSPredicate(format: predicate)
        if !waitUntil(timeout: timeout, { condition.evaluate(with: object) }) {
            XCTFail(message, file: file, line: line)
        }
    }

    // MARK: Moving

    /// Scrolls until the element can be tapped and is not under the keyboard.
    @discardableResult
    func scrollTo(_ identifier: String, file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let found = element(identifier)
        // A list builds its rows lazily: one far down doesn't exist until
        // scrolled to. Give it a moment to appear, then go looking. Each
        // look at an element costs about half a second on CI, so none is
        // repeated: `exists` is known from the wait, then from each swipe.
        var exists = waitToExist(found, timeout: 5)
        var swipes = 0
        while !(exists && found.isHittable && !underKeyboard(found)) && swipes < 8 {
            if exists && found.frame.maxY < app.frame.height * 0.2 {
                app.swipeDown()
            } else {
                app.swipeUp()
            }
            swipes += 1
            exists = found.exists
        }
        if !exists {
            exists = waitToExist(found, timeout: Self.patience)
        }
        XCTAssertTrue(exists, "\"\(identifier)\" should be on screen", file: file, line: line)
        if swipes > 0 {
            waitUntilStill(found)
        }
        return found
    }

    /// A swipe leaves the list coasting, and a tap on a coasting list only
    /// stops it: wait until the element is in the same place twice running.
    private func waitUntilStill(_ found: XCUIElement) {
        var last = found.frame
        for _ in 0..<20 {
            RunLoop.current.run(until: Date().addingTimeInterval(0.15))
            let now = found.frame
            if now == last { return }
            last = now
        }
    }

    /// The keyboard covers it, though it counts as hittable.
    func underKeyboard(_ found: XCUIElement) -> Bool {
        let keyboard = app.keyboards.firstMatch
        return keyboard.exists && found.frame.maxY > keyboard.frame.minY
    }

    func tap(_ identifier: String, file: StaticString = #filePath, line: UInt = #line) {
        scrollTo(identifier, file: file, line: line).tap()
    }

    func goBack() {
        let back = app.navigationBars.buttons["BackButton"]
        if back.exists {
            back.tap()
        } else {
            app.navigationBars.buttons.element(boundBy: 0).tap()
        }
    }

    /// paw to the background and back. With `-pawSimAwayMs` the simulated
    /// clock moves on while it is away.
    func backgroundAndReturn() {
        // The Home button press is now and then lost on a busy simulator:
        // press again until paw has left the foreground.
        for _ in 0..<3 where app.state == .runningForeground {
            XCUIDevice.shared.press(.home)
            let deadline = Date().addingTimeInterval(10)
            while app.state == .runningForeground && Date() < deadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.2))
            }
        }
        XCTAssertNotEqual(app.state, .runningForeground, "paw should have gone to the background")
        app.activate()
        XCTAssertTrue(waitUntil(timeout: Self.patience) { app.state == .runningForeground }, "paw should be back")
    }

    // MARK: The key sheet

    /// The PIN field, waited for.
    @discardableResult
    func pinField(file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let field = element("pin-field")
        XCTAssertTrue(waitToExist(field, timeout: Self.patience), "The key sheet should ask for the PIN",
                      file: file, line: line)
        return field
    }

    /// Types `pin` into the PIN field once it has the keyboard, checks that
    /// every character arrived (the field shows one dot per character),
    /// and sends it with the return key, which cannot miss the way a tap on
    /// Continue can while the sheet is still resizing. `send: false` leaves
    /// it typed.
    func enterPin(_ pin: String, send: Bool = true, file: StaticString = #filePath, line: UInt = #line) {
        let field = pinField(file: file, line: line)
        for _ in 0..<3 {
            field.tap()
            XCTAssertTrue(waitToExist(app.keyboards.firstMatch, timeout: Self.patience),
                          "The keyboard should come up for the PIN", file: file, line: line)
            clear(field)
            field.typeText(pin)
            if typedLength(field) == pin.count { break }
        }
        XCTAssertEqual(typedLength(field), pin.count, "Every character of the PIN should have arrived",
                       file: file, line: line)
        if send {
            field.typeText("\n")
        }
    }

    /// How many characters the PIN field holds: it shows dots, or the PIN
    /// itself once Show is on; empty shows the placeholder.
    private func typedLength(_ field: XCUIElement) -> Int {
        guard let value = field.value as? String, value != "Key PIN" else { return 0 }
        return value.count
    }

    private func clear(_ field: XCUIElement) {
        let count = typedLength(field)
        if count > 0 {
            field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: count))
        }
    }

    /// A simulator control drawn below the key sheet's own buttons.
    func tapSimulated(_ identifier: String, file: StaticString = #filePath, line: UInt = #line) {
        let button = app.buttons[identifier]
        XCTAssertTrue(waitToExist(button, timeout: Self.patience), "\"\(identifier)\" should be on screen",
                      file: file, line: line)
        button.tap()
    }

    /// The touch prompt is up: the simulated finger goes on the key.
    func touchTheKey(file: StaticString = #filePath, line: UInt = #line) {
        expect("touch-prompt", file: file, line: line)
        tapSimulated("simulated-touch", file: file, line: line)
    }

    /// The key's touch timeout runs out now, from the sheet's simulator control.
    func timeOutTheTouch(file: StaticString = #filePath, line: UInt = #line) {
        expect("touch-prompt", file: file, line: line)
        tapSimulated("simulated-touch-timeout", file: file, line: line)
    }

    /// Waits until every key task the app started has ended, including any
    /// the sheet let go of (Cancel, Lock now), as the app's debug element
    /// `sim-key-tasks` reports. Only then does "nothing opened" mean anything.
    func waitForKeyTasksToEnd(file: StaticString = #filePath, line: UInt = #line) {
        let tasks = element("sim-key-tasks")
        XCTAssertTrue(waitToExist(tasks, timeout: Self.patience), "The app should report its key tasks",
                      file: file, line: line)
        wait(for: "value == '0'", on: tasks, timeout: Self.patience,
             "Every key task should have ended, \(String(describing: tasks.value)) still running",
             file: file, line: line)
    }

    // MARK: Settings and the simulator's controls there

    func openSettings() {
        tap("settings-button")
        expect("settings-lock-now")
    }

    func closeSettings() {
        app.buttons["settings-done"].tap()
        expectGone("settings-done")
    }

    /// Opens the Simulator row at the foot of Settings: its controls are
    /// closed until it is tapped.
    func showSimulatorControls(file: StaticString = #filePath, line: UInt = #line) {
        let row = scrollTo("sim-controls", file: file, line: line)
        if (row.value as? String) != "shown" {
            row.tap()
        }
    }

    /// Taps a control in Settings' Simulator section, then closes Settings.
    func inSettings(tapping identifiers: String...) {
        openSettings()
        showSimulatorControls()
        for identifier in identifiers {
            tap(identifier)
        }
        closeSettings()
    }

    /// What paw's pasteboard holds now, read inside the app (no paste prompt).
    func pasteboardContents(file: StaticString = #filePath, line: UInt = #line) -> String {
        openSettings()
        showSimulatorControls(file: file, line: line)
        tap("sim-read-pasteboard", file: file, line: line)
        let shown = expect("sim-pasteboard-value", file: file, line: line)
        let value = (shown.value as? String) ?? ""
        closeSettings()
        return value
    }

    /// From Home: the web folder, then github.com with PIN and touch.
    func openGitHubWithKey(askingForPin: Bool = true) {
        tap("folder-web")
        tap("entry-github.com")
        expectLabel("key-sheet-purpose", contains: "Open github.com")
        if askingForPin {
            enterPin(Self.pin)
        }
        touchTheKey()
        expectLabel("entry-title", contains: "github.com")
    }
}
