import XCTest
@testable import Paw

/// The key-task driver every key sheet uses, on its own: the core's states
/// as sheet stages, the PIN field, each task's end heard exactly once, and
/// a task's end after Cancel. Real tasks on the fixture's simulated key.
@MainActor
final class KeyTaskDriverTests: XCTestCase {
    /// Words and ends, as a workflow would have them, recorded.
    @MainActor
    private final class Owner: KeyTaskOwner {
        var ends: [KeyTaskEnd] = []
        var lateEnds: [KeyTaskEnd] = []
        var doneShownCount = 0

        let keyWords = KeyErrorWords.Context(entry: "github.com")

        func workingStage(_ progress: TaskProgress?) -> KeySheetStage {
            .working(message: "Opening github.com\u{2026}", progress: nil)
        }

        func keyTaskEnded(_ end: KeyTaskEnd) -> KeySheetStage? {
            ends.append(end)
            if case .done(.opened(let entry, _, _)) = end.result { entry.close() }
            return standardStage(end.result)
        }

        func letGoTaskEnded(_ end: KeyTaskEnd) {
            lateEnds.append(end)
            if case .done(.opened(let entry, _, _)) = end.result { entry.close() }
        }

        func doneShown() {
            doneShownCount += 1
        }
    }

    private func driver(owner: Owner) -> KeyTaskDriver {
        let driver = KeyTaskDriver()
        driver.owner = owner
        return driver
    }

    private func open(_ path: String, with driver: KeyTaskDriver, _ fixture: Fixture) {
        let store = fixture.model.store
        let port = fixture.model.keyPort
        driver.start { listener in
            port.openEntry(store: store, entry: path, copies: nil, ttlSecs: 0, listener: listener)
        }
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
            XCTAssertNil(KeyTaskResult(state), "\(state)")
            XCTAssertFalse(state.isFinal, "\(state)")
        }
        let ended: [KeyTaskState] = [
            .done(outcome: .signedIn), .failed(error: .TouchTimeout), .syncFailed(error: .KeyRejected),
            .setupFailed(error: .EmptyRepository), .cancelled, .failed(error: .Cancelled),
        ]
        for state in ended {
            XCTAssertTrue(state.isFinal, "\(state)")
        }
        guard case .cancelled? = KeyTaskResult(.failed(error: .Cancelled)) else {
            return XCTFail("The key's Cancelled is a cancel")
        }
    }

    func testTheStageForEachEnd() {
        let owner = Owner()
        XCTAssertEqual(owner.standardStage(.done(.signedIn)), .done)
        XCTAssertEqual(owner.standardStage(.failed(.PinBlocked)), .locked)
        XCTAssertEqual(owner.standardStage(.failed(.TouchTimeout)), .failed(
            problem: "Your key wasn't touched in time", nextStep: "Try again and touch the gold part while it blinks."))
        XCTAssertNil(owner.standardStage(.cancelled))
        XCTAssertNil(owner.standardStage(.syncFailed(.KeyRejected)))
        XCTAssertNil(owner.standardStage(.setupFailed(.EmptyRepository)))
    }

    func testOnlyThePinStagesAskForThePin() {
        XCTAssertTrue(KeySheetStage.pin(triesLeft: 3, forgotten: false).asksForPin)
        XCTAssertTrue(KeySheetStage.wrongPin(triesLeft: 2).asksForPin)
        XCTAssertTrue(KeySheetStage.pinAgain(triesLeft: 3, problem: "", nextStep: "").asksForPin)
        for stage: KeySheetStage in [.plugIn, .busy, .touch, .working(message: "", progress: nil), .done, .locked,
                                     .failed(problem: "", nextStep: "")] {
            XCTAssertFalse(stage.asksForPin, "\(stage)")
        }
    }

    func testThePinStagesAndThePinField() async throws {
        let fixture = try Fixture()
        let owner = Owner()
        let driver = driver(owner: owner)
        open("web/github.com", with: driver, fixture)
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
        let owner = Owner()
        let driver = driver(owner: owner)
        driver.generation = 4
        open("web/github.com", with: driver, fixture)
        await waitForStage(driver, .pin(triesLeft: 3, forgotten: false))
        driver.pin = Fixture.pin
        driver.continueWithPin()
        await waitForStage(driver, .done)
        XCTAssertEqual(owner.ends.count, 1)
        guard case .done(.opened)? = owner.ends.first?.result else {
            return XCTFail("The open should end done, it ended \(String(describing: owner.ends.first?.result))")
        }
        XCTAssertEqual(owner.ends.first?.purpose, .openEntry(entry: "web/github.com"))
        XCTAssertEqual(owner.ends.first?.generation, 4)
        XCTAssertFalse(driver.isFollowing)
        XCTAssertEqual(driver.pendingTasks, 0)

        await waitFor("Done to have shown for a second") { owner.doneShownCount == 1 }
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertEqual(owner.ends.count, 1, "Heard once")
        XCTAssertEqual(owner.lateEnds.count, 0)
        XCTAssertEqual(owner.doneShownCount, 1)
    }

    func testATaskThatEndsAfterCancelIsHeardOnceAsLetGo() async throws {
        let fixture = try Fixture(touch: true)
        let owner = Owner()
        let driver = driver(owner: owner)
        open("web/github.com", with: driver, fixture)
        await waitForStage(driver, .pin(triesLeft: 3, forgotten: false))
        driver.pin = Fixture.pin
        driver.continueWithPin()
        await waitFor("the touch") { driver.stage == .touch && fixture.key.waitingForTouch() }

        driver.cancel()
        XCTAssertFalse(driver.isFollowing)
        // The key is touched after the cancel: the core throws the answer away.
        fixture.key.touch()
        await waitFor("the cancelled task to end") { driver.pendingTasks == 0 }
        XCTAssertEqual(owner.ends.count, 0, "Nothing reaches the sheet after Cancel")
        XCTAssertEqual(owner.lateEnds.count, 1)
        XCTAssertEqual(owner.doneShownCount, 0)
    }

    func testCancelEndsTheTaskAtOnceWhileTheKeyStillWaitsForATouch() async throws {
        let fixture = try Fixture(touch: true)
        let owner = Owner()
        let driver = driver(owner: owner)
        let port = fixture.model.keyPort
        driver.start { listener in port.checkTouchTimeout(listener: listener) }
        await waitForStage(driver, .pin(triesLeft: 3, forgotten: false))
        driver.pin = Fixture.pin
        driver.continueWithPin()
        await waitFor("the touch") { driver.stage == .touch && fixture.key.waitingForTouch() }

        driver.cancel()
        XCTAssertFalse(driver.isFollowing)
        guard case .cancelled? = driver.state else {
            return XCTFail("Cancel should end the check at once, the state is \(String(describing: driver.state))")
        }
        XCTAssertTrue(fixture.key.waitingForTouch(), "The key still waits for a touch")
        XCTAssertEqual(driver.pendingTasks, 0)
        XCTAssertEqual(owner.lateEnds.count, 1)

        // The key is touched after the cancel: the core throws the answer away.
        fixture.key.touch()
        await waitFor("the key to stop waiting") { !fixture.key.waitingForTouch() }
        guard case .cancelled? = driver.state else {
            return XCTFail("The key's late answer should change nothing, the state is \(String(describing: driver.state))")
        }
        XCTAssertEqual(owner.lateEnds.count, 1)
        XCTAssertEqual(owner.ends.count, 0)
    }

    func testALateFinishAfterCancelIsHeardAsACancelThenAsLetGo() async throws {
        let fixture = try Fixture()
        let owner = Owner()
        let driver = driver(owner: owner)
        driver.simulatesLateCancelNext = true
        open("web/github.com", with: driver, fixture)
        await waitForStage(driver, .pin(triesLeft: 3, forgotten: false))
        driver.pin = Fixture.pin
        driver.continueWithPin()
        await waitFor("the open to end") { driver.pendingTasks == 0 }
        guard case .cancelled? = owner.ends.first?.result, owner.ends.count == 1 else {
            return XCTFail("The sheet should hear one cancel, it heard \(owner.ends.map(\.result))")
        }
        guard case .done(.opened)? = owner.lateEnds.first?.result, owner.lateEnds.count == 1 else {
            return XCTFail("The open should end late, once, it ended \(owner.lateEnds.map(\.result))")
        }
    }

    func testAKeyThatCannotBeReadShowsOnPlugInAndTryAgainReadsItAgain() async throws {
        let fixture = try Fixture()
        let owner = Owner()
        let key = fixture.key
        let reads = FailingReads(1000)
        let port = KeyPort(clock: fixture.clock.asClock())
        let reader = KeyReader(port: port) {
            if reads.take() {
                throw ConnectionError.Failed(message: "The key didn't answer while it was being read.")
            }
            return key.asConnection()
        }
        reader.watch(startsUnplugged: false)
        let driver = KeyTaskDriver(reader: reader)
        driver.owner = owner
        let couldNotRead = KeySheetStage.failed(
            problem: "paw couldn't read your key",
            nextStep: "The key didn't answer while it was being read. Try again, or unplug the key and plug it back in.")

        driver.start { listener in port.checkTouchTimeout(listener: listener) }
        await waitForStage(driver, couldNotRead)
        XCTAssertNotNil(driver.readProblem)

        driver.retry()
        XCTAssertEqual(driver.stage, .plugIn, "Reading again")
        await waitForStage(driver, couldNotRead)

        reads.stop()
        driver.retry()
        await waitForStage(driver, .pin(triesLeft: 3, forgotten: false))
        XCTAssertNil(driver.readProblem)
        driver.cancel()
    }
}
