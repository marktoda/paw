import Foundation
import XCTest
@testable import Paw

/// The app's model on the core's fixture store, in-process, as
/// `FixtureSession` opens it for the UI tests, but with no screens and a
/// simulated key of the test's own (no touch unless `touch`), whose first
/// `failingReads` reads fail. `writable` sets the store up through the
/// simulated git host, so it can be written and synced.
///
/// One class per file in this target, named as the file: scripts/ci-test.sh
/// finds a unit test class named in `PAW_ONLY` by its file.
@MainActor
final class Fixture {
    static let pin = FixtureSession.pin
    static let githubPassword = "tidal-Quartz-88-ferns"

    let key: SimulatedKey
    /// The key as the platform has it: "finish the next open after
    /// Cancel", and "let the key answer" after reads that failed.
    let keys: SimulatedKeyConnector
    let clock: SimClock
    let keystore: SimKeystore
    let model: AppModel
    let touchRequired: Bool

    init(touch: Bool = false, failingReads: Int = 0, writable: Bool = false) throws {
        let key = SimulatedKey(pin: Self.pin, touchRequired: touch, touchTimeoutMs: 15_000)
        let hardware = FixtureSession.OwnHardware(key: key, failingReads: failingReads)
        let fixture = try FixtureSession(on: hardware.platform, writable: writable)
        self.key = key
        keys = hardware.keys
        clock = hardware.clock
        keystore = hardware.keystore
        model = AppModel(session: fixture.session)
        touchRequired = touch
    }

    /// The key sheet's stage, `nil` with no sheet.
    var stage: KeySheetStage? {
        model.keyRequest == nil ? nil : model.keyTask.stage
    }

    /// The opened entry the stack shows last.
    var shownEntry: UUID? {
        if case .entry(let id)? = model.path.last { return id }
        return nil
    }

    func waitForStage(_ stage: KeySheetStage, file: StaticString = #filePath, line: UInt = #line) async {
        await waitFor("the key sheet at \(stage), it is at \(String(describing: self.stage))", file: file, line: line) {
            self.stage == stage
        }
    }

    /// Waits for the key to blink and touches it. `false` if the PIN was
    /// asked for on the way.
    @discardableResult
    func touchWhenAsked(file: StaticString = #filePath, line: UInt = #line) async -> Bool {
        var askedForPin = false
        await waitFor("the key to wait for a touch", file: file, line: line) {
            if case .pin? = self.stage { askedForPin = true }
            return self.stage == .touch && self.key.waitingForTouch()
        }
        key.touch()
        return !askedForPin
    }

    func enterPin(_ pin: String = Fixture.pin) {
        model.keyTask.pin = pin
        model.keyTask.continueWithPin()
    }

    /// The sheet at Done goes away, as the screen takes it away, and the
    /// entry is pushed.
    func dismissDoneSheet(file: StaticString = #filePath, line: UInt = #line) async {
        await waitForStage(.done, file: file, line: line)
        model.cancelKeyTask()
        model.keySheetDismissed()
    }

    /// A tap on the entry's row, then the key sheet to the end: the PIN if
    /// asked for, a touch if the key needs one. Returns the open entry's id.
    @discardableResult
    func open(_ path: String, askingForPin: Bool = true,
              file: StaticString = #filePath, line: UInt = #line) async -> UUID? {
        model.open(EntryRef(path: path))
        if askingForPin {
            await waitForStage(.pin(triesLeft: 3, forgotten: false), file: file, line: line)
            enterPin()
        }
        if touchRequired {
            let noPin = await touchWhenAsked(file: file, line: line)
            if !askingForPin {
                XCTAssertTrue(noPin, "The PIN should still be held", file: file, line: line)
            }
        }
        await dismissDoneSheet(file: file, line: line)
        guard let id = shownEntry, model.session(id)?.ref.path == path else {
            XCTFail("\(path) should be open, the stack is \(model.path)", file: file, line: line)
            return nil
        }
        return id
    }

    /// Back to Home, as the back button does.
    func goHome() {
        model.setPath([])
    }

    /// A new entry with a typed password, saved as the form's Save does once
    /// it knows who can open the folder. Needs a `writable` fixture.
    func add(_ name: String, folder: String) async throws {
        try await FixtureSession.add(name, folder: folder, in: model)
    }
}

/// Lets the main queue run (the core's listeners, the card queue's answers)
/// until `condition` holds, or fails the test after `timeout` seconds.
@MainActor
@discardableResult
func waitFor(_ what: @autoclosure () -> String, timeout: TimeInterval = 15,
             file: StaticString = #filePath, line: UInt = #line, until condition: () -> Bool) async -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    while !condition() {
        if Date() > deadline {
            XCTFail("Waited \(Int(timeout)) s for \(what())", file: file, line: line)
            return false
        }
        try? await Task.sleep(for: .milliseconds(10))
    }
    return true
}
