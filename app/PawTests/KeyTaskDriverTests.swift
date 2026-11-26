import XCTest
@testable import Paw

/// The key-task driver every key sheet uses, on its own: the core's states
/// as sheet stages, the PIN field, each task's end heard exactly once
/// (and the after-end hook with it), a task's end after Cancel, what each
/// task holds until it ends, and a key that drops out mid-task. Real tasks on the fixture's simulated key.
@MainActor
final class KeyTaskDriverTests: XCTestCase {
    /// Background time as a task holds it: how many times it was ended.
    @MainActor
    private final class Hold: KeyTaskHold {
        var ends = 0

        func end() {
            ends += 1
        }
    }

    /// A workflow's words, ends and holds, recorded.
    @MainActor
    private final class Recorder {
        var ends: [KeyTaskEnd<Never>] = []
        var lateEnds: [KeyTaskEnd<Never>] = []
        var afterEnds = 0
        var holds: [Hold] = []
        var closedCount = 0
        let words = KeyErrorWords.Context(entry: "github.com")

        func job(_ begin: @escaping (KeyTaskListener) -> KeyTask) -> KeyJob<Never> {
            KeyJob(
                begin: begin,
                words: { [words] in words },
                working: { _ in ("Opening github.com\u{2026}", nil) },
                ended: { [unowned self] end in
                    ends.append(end)
                    if case .done(.opened(let entry, _, _)) = end { entry.close() }
                    return .standard
                },
                letGo: { [unowned self] end in
                    lateEnds.append(end)
                    if case .done(.opened(let entry, _, _)) = end { entry.close() }
                },
                afterEnd: { [unowned self] in
                    afterEnds += 1
                },
                hold: { [unowned self] in
                    let hold = Hold()
                    holds.append(hold)
                    return hold
                },
                closed: { [unowned self] in
                    closedCount += 1
                })
        }
    }

    /// The next `count` card exchanges fail, as when the key drops off
    /// the port for a moment.
    private final class Dropouts: @unchecked Sendable {
        private let lock = NSLock()
        private var left = 0

        func failNext(_ count: Int) {
            lock.withLock { left = count }
        }

        func take() -> Bool {
            lock.withLock {
                guard left > 0 else { return false }
                left -= 1
                return true
            }
        }
    }

    private final class DroppingConnection: CardConnection, @unchecked Sendable {
        let key: CardConnection
        let dropouts: Dropouts

        init(key: CardConnection, dropouts: Dropouts) {
            self.key = key
            self.dropouts = dropouts
        }

        func transmit(apdu: Data) throws -> Data {
            if dropouts.take() {
                throw ConnectionError.Failed(message: "The key went away.")
            }
            return try key.transmit(apdu: apdu)
        }
    }

    private func driver(_ fixture: Fixture) -> KeyTaskDriver {
        fixture.model.storeSession.keyTasks.make()
    }

    private func open(_ path: String, with driver: KeyTaskDriver, _ recorder: Recorder, _ fixture: Fixture) {
        let store = fixture.model.store
        let port = fixture.model.storeSession.keyPort
        driver.start(recorder.job { listener in
            port.openEntry(store: store, entry: path, copies: nil, ttlSecs: 0, listener: listener)
        })
    }

    private func waitForStage(_ driver: KeyTaskDriver, _ stage: KeySheetStage,
                              file: StaticString = #filePath, line: UInt = #line) async {
        await waitFor("the stage \(stage), it is \(driver.stage)", file: file, line: line) { driver.stage == stage }
    }

    func testEveryFinalStateIsAResultAndNoOtherIs() {
        let running: [KeyTaskState] = [
            .plugIn, .pin(triesLeft: 3, forgotAfterBackground: false, lastError: nil), .busy, .touch,
            .working(progress: nil),
        ]
        for state in running {
            XCTAssertNil(KeyTaskEnd<SyncError>(state), "\(state)")
            XCTAssertFalse(state.isFinal, "\(state)")
        }
        let rejected = SyncError.Fault(fault: .remote(failure: .keyNotAccepted(host: "h", publicKey: "k")))
        let ended: [KeyTaskState] = [
            .done(outcome: .signedIn), .failed(error: .TouchTimeout), .syncFailed(error: rejected),
            .setupFailed(error: .EmptyRepository), .cancelled, .failed(error: .Cancelled),
        ]
        for state in ended {
            XCTAssertTrue(state.isFinal, "\(state)")
        }
        guard case .cancelled? = KeyTaskEnd<Never>(.failed(error: .Cancelled)) else {
            return XCTFail("The key's Cancelled is a cancel")
        }
        guard case .stepFailed? = KeyTaskEnd<SyncError>(.syncFailed(error: rejected)),
              case .stepFailed? = KeyTaskEnd<SetupError>(.setupFailed(error: .EmptyRepository)) else {
            return XCTFail("A sync's or a setup's own failure is its step's")
        }
    }

    func testTheStageForEachEnd() {
        let words = KeyErrorWords.Context(entry: "github.com")
        XCTAssertEqual(KeyTaskEnd<Never>.done(.signedIn).standardStage(words), .done)
        XCTAssertEqual(KeyTaskEnd<Never>.failed(.PinBlocked).standardStage(words), .locked)
        XCTAssertEqual(KeyTaskEnd<Never>.failed(.TouchTimeout).standardStage(words), .failed(
            problem: "Your key wasn't touched in time", nextStep: "Try again and touch the gold part while it blinks."))
        XCTAssertNil(KeyTaskEnd<Never>.cancelled.standardStage(words))
        let rejected = SyncError.Fault(fault: .remote(failure: .keyNotAccepted(host: "h", publicKey: "k")))
        XCTAssertNil(KeyTaskEnd<SyncError>.stepFailed(rejected).standardStage(words))
        XCTAssertNil(KeyTaskEnd<SetupError>.stepFailed(.EmptyRepository).standardStage(words))
    }

    func testThePinStagesAndThePinField() async throws {
        let fixture = try Fixture()
        let owner = Recorder()
        let driver = self.driver(fixture)
        open("web/github.com", with: driver, owner, fixture)
        await waitForStage(driver, .pin(triesLeft: 3, forgotten: false))

        driver.pin = "000000"
        driver.continueWithPin()
        XCTAssertEqual(driver.pin, "", "Handed to the core and cleared at once")
        await waitForStage(driver, .wrongPin(triesLeft: 2))

        driver.pin = "12345"
        driver.continueWithPin()
        await waitForStage(driver, .pinAgain(
            triesLeft: 2, problem: "A PIN has 6 to 127 characters", nextStep: "Enter it again; no try was used."))

        // Typed and not sent: cleared once the sheet stops asking for it.
        driver.pin = "1234"
        driver.retry()
        XCTAssertEqual(driver.pin, "")
        await waitForStage(driver, .pin(triesLeft: 2, forgotten: false))

        driver.pin = Fixture.pin
        driver.continueWithPin()
        await waitForStage(driver, .done)
        XCTAssertEqual(fixture.key.pinTriesLeft(), 3)
    }

    func testEachEndIsHeardOnceAndDoneClosesAfterASecond() async throws {
        let fixture = try Fixture()
        let owner = Recorder()
        let driver = self.driver(fixture)
        open("web/github.com", with: driver, owner, fixture)
        await waitForStage(driver, .pin(triesLeft: 3, forgotten: false))
        driver.pin = Fixture.pin
        driver.continueWithPin()
        await waitForStage(driver, .done)
        XCTAssertEqual(owner.ends.count, 1)
        guard case .done(.opened)? = owner.ends.first else {
            return XCTFail("The open should end done, it ended \(String(describing: owner.ends.first))")
        }
        XCTAssertEqual(driver.taskPurpose, .openEntry(entry: "web/github.com"))
        XCTAssertFalse(driver.isFollowing)
        XCTAssertEqual(driver.pendingTasks, 0)
        XCTAssertTrue(driver.isShown, "Done shows")

        await waitFor("Done to have shown for a second") { !driver.isShown }
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertEqual(owner.ends.count, 1, "Heard once")
        XCTAssertEqual(owner.lateEnds.count, 0)
        XCTAssertEqual(owner.afterEnds, 1, "After the followed end")
        XCTAssertEqual(owner.closedCount, 1)
    }

    func testCancelAllLetsGoOfEveryTaskAndSaysSoToEach() async throws {
        let fixture = try Fixture(touch: true)
        let owner = Recorder()
        let driver = self.driver(fixture)
        open("web/github.com", with: driver, owner, fixture)
        await waitForStage(driver, .pin(triesLeft: 3, forgotten: false))
        driver.pin = Fixture.pin
        driver.continueWithPin()
        await waitFor("the touch") { driver.stage == .touch && fixture.key.waitingForTouch() }

        driver.cancelAll()
        XCTAssertFalse(driver.isShown)
        fixture.key.touch()
        await waitFor("the let-go task to end") { driver.pendingTasks == 0 }
        XCTAssertEqual(owner.ends.count, 0)
        XCTAssertEqual(owner.lateEnds.count, 1)
        XCTAssertEqual(owner.afterEnds, 1, "After the let-go end too")
    }

    func testEachTaskKeepsItsHoldUntilItHasReallyEnded() async throws {
        let fixture = try Fixture(touch: true)
        let owner = Recorder()
        let driver = self.driver(fixture)
        open("web/github.com", with: driver, owner, fixture)
        await waitForStage(driver, .pin(triesLeft: 3, forgotten: false))
        driver.pin = Fixture.pin
        driver.continueWithPin()
        await waitFor("the touch") { driver.stage == .touch && fixture.key.waitingForTouch() }
        XCTAssertEqual(owner.holds.count, 1, "Taken as the task started")
        XCTAssertEqual(owner.holds[0].ends, 0, "Kept while the task runs")

        fixture.key.touch()
        await waitForStage(driver, .done)
        XCTAssertEqual(owner.holds[0].ends, 1, "Ended with the task, once")

        // A task the sheet lets go of keeps its own until it ends too.
        await waitFor("Done to close") { !driver.isShown }
        open("web/github.com", with: driver, owner, fixture)
        await waitFor("the second touch") { driver.stage == .touch && fixture.key.waitingForTouch() }
        driver.retry()
        XCTAssertEqual(owner.holds.count, 3, "Try again's task took its own")
        XCTAssertEqual(owner.holds[1].ends, 1, "Let go by Try again: its task ended at once")
        XCTAssertEqual(owner.holds[2].ends, 0)
        driver.cancel()
        XCTAssertEqual(owner.holds[2].ends, 1)
        XCTAssertEqual(owner.holds.map(\.ends), [1, 1, 1], "Each ended once")
    }

    func testTheSignalCountsEveryDriversTasks() async throws {
        let fixture = try Fixture(touch: true)
        let keyTasks = fixture.model.storeSession.keyTasks
        let other = keyTasks.make()
        let owner = Recorder()
        fixture.model.open(EntryRef(path: "web/github.com"))
        await fixture.waitForStage(.pin(triesLeft: 3, forgotten: false))
        open("email/fastmail", with: other, owner, fixture)
        XCTAssertEqual(fixture.model.keyTask.pendingTasks, 1)
        XCTAssertEqual(other.pendingTasks, 1)
        XCTAssertEqual(keyTasks.pendingTasks, 2, "The app's open and the other driver's task")

        fixture.model.cancelKeyTask()
        other.cancel()
        await waitFor("both tasks to end") { keyTasks.pendingTasks == 0 }
    }

    func testCancelEndsTheTaskAtOnceWhileTheKeyStillWaitsForATouch() async throws {
        let fixture = try Fixture(touch: true)
        let owner = Recorder()
        let driver = self.driver(fixture)
        let port = fixture.model.storeSession.keyPort
        driver.start(owner.job { listener in port.checkTouchTimeout(listener: listener) })
        await waitForStage(driver, .pin(triesLeft: 3, forgotten: false))
        driver.pin = Fixture.pin
        driver.continueWithPin()
        await waitFor("the touch") { driver.stage == .touch && fixture.key.waitingForTouch() }

        driver.cancel()
        XCTAssertFalse(driver.isFollowing)
        guard case .cancelled? = owner.lateEnds.first, owner.lateEnds.count == 1 else {
            return XCTFail("Cancel should end the check at once, it ended \(owner.lateEnds)")
        }
        XCTAssertTrue(fixture.key.waitingForTouch(), "The key still waits for a touch")
        XCTAssertEqual(driver.pendingTasks, 0)
        XCTAssertFalse(driver.isShown)
        XCTAssertEqual(owner.closedCount, 1)

        // The key is touched after the cancel: the core throws the answer away.
        fixture.key.touch()
        await waitFor("the key to stop waiting") { !fixture.key.waitingForTouch() }
        XCTAssertEqual(owner.lateEnds.count, 1, "The key's late answer changes nothing")
        XCTAssertEqual(owner.ends.count, 0)
        XCTAssertEqual(owner.afterEnds, 1)
        XCTAssertNil(driver.lastEnd, "Not the sheet's end")
    }

    func testAKeyThatDropsOutMidTaskIsReadAgainSoTryAgainCarriesOn() async throws {
        let fixture = try Fixture()
        let owner = Recorder()
        let key = fixture.key
        let dropouts = Dropouts()
        let port = KeyPort(clock: fixture.clock.asClock())
        let reader = KeyReader(port: port, source: SimulatedKeyConnector(connect: {
            DroppingConnection(key: key.asConnection(), dropouts: dropouts)
        }))
        reader.watch()
        let driver = KeyTaskDriver(reader: reader)
        driver.start(owner.job { listener in port.checkTouchTimeout(listener: listener) })
        await waitForStage(driver, .pin(triesLeft: 3, forgotten: false))

        dropouts.failNext(1)
        driver.pin = Fixture.pin
        driver.continueWithPin()
        await waitFor("the task to end with the key gone, it ended \(owner.ends)") {
            if case .failed(.KeyRemoved)? = owner.ends.last { return true }
            return false
        }
        // Still in, as far as the system knows: read again, with no replug.
        await waitFor("the key to be read again") { port.session() != nil }

        driver.retry()
        await waitForStage(driver, .pin(triesLeft: 3, forgotten: false))
        driver.pin = Fixture.pin
        driver.continueWithPin()
        await waitForStage(driver, .done)
        XCTAssertEqual(owner.ends.count, 2)
    }
}
