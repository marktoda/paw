import Foundation
import XCTest
@testable import Paw

/// The app's model on the core's fixture store, in-process: a simulated key
/// of its own (PIN 123456), a simulated keystore that answers as Face ID
/// and a simulated clock, as `FixtureLaunch` builds them for the UI tests,
/// but with no screens, built through the same `StoreSession` as the app.
/// `writable` sets the store up through the simulated git host
/// (`WritableFixture`), so it can be written and synced.
///
/// One class per file in this target, named as the file: scripts/ci-test.sh
/// finds a unit test class named in `PAW_ONLY` by its file.
@MainActor
final class Fixture {
    static let pin = "123456"
    static let githubPassword = "tidal-Quartz-88-ferns"

    let key: SimulatedKey
    let clock: SimClock
    let keystore: SimKeystore
    /// Reads of the key that fail before one answers.
    let reads: FailingReads
    let model: AppModel
    let touchRequired: Bool

    init(touch: Bool = false, failingReads: Int = 0, writable: Bool = false) throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("paw-unit-\(UUID().uuidString)", isDirectory: true)
        let copiesDir = root.appendingPathComponent("window", isDirectory: true)
        try FileManager.default.createDirectory(at: copiesDir, withIntermediateDirectories: true)
        var prepared: WritableFixture.Prepared?
        let storePath: String
        if writable {
            let found = try WritableFixture.prepare(root: root)
            prepared = found
            storePath = found.repoDir
        } else {
            storePath = root.appendingPathComponent("store", isDirectory: true).path
            try writeFixtureStore(dir: storePath)
        }

        let key = SimulatedKey(pin: "123456", touchRequired: touch, touchTimeoutMs: 15_000)
        let clock = SimClock(wallMs: Int64(Date().timeIntervalSince1970 * 1000))
        let keystore = SimKeystore()
        let reads = FailingReads(failingReads)
        let shared = clock.asClock()
        let copies = CopyCache(dir: copiesDir.path, keystore: keystore.asKeystore(), clock: shared)
        let session = try StoreSession(StoreSession.Parts(
            repoDir: storePath, copies: copies, clock: shared, keyPort: KeyPort(clock: shared),
            connect: {
                if reads.take() {
                    throw ConnectionError.Failed(message: "The key didn't answer while it was being read.")
                }
                return key.asConnection()
            },
            writing: prepared?.writing, gitHub: nil, setUp: nil))
        let model = AppModel(session: session)
        self.key = key
        self.clock = clock
        self.keystore = keystore
        self.reads = reads
        self.model = model
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

    func enterPin(_ pin: String = "123456") {
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
    func add(_ name: String, folder: String, file: StaticString = #filePath, line: UInt = #line) async throws {
        let edits = try XCTUnwrap(model.edits, "The fixture store should be writable", file: file, line: line)
        let form = EntryFormModel(
            adding: folder, writer: edits.writer, folders: model.allFolders, live: model.storeSession.live)
        form.name = name
        form.password = "Typed-pass-42"
        form.changed()
        await waitFor("who can open \(folder)", file: file, line: line) { form.folderWritable != nil }
        XCTAssertTrue(form.canSave, "\(form.problems)", file: file, line: line)
        XCTAssertTrue(model.save(form), "\(String(describing: form.saveProblem))", file: file, line: line)
    }
}

/// The first `count` reads of the key fail, as `-pawSimAttachFails` has it
/// for the UI tests, until `stop`.
final class FailingReads: @unchecked Sendable {
    private let lock = NSLock()
    private var left: Int

    init(_ count: Int) {
        left = count
    }

    func take() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard left > 0 else { return false }
        left -= 1
        return true
    }

    func stop() {
        lock.lock()
        left = 0
        lock.unlock()
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
