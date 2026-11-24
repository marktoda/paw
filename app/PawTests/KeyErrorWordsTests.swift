import XCTest
@testable import Paw

/// What the key sheet says for each of the core's key errors.
final class KeyErrorWordsTests: XCTestCase {
    func testAWrongPinCountsTheTriesLeft() {
        XCTAssertEqual(
            KeyErrorWords(.WrongPin(triesLeft: 2), entry: "github.com"),
            KeyErrorWords(problem: "Wrong PIN", nextStep: "2 tries left."))
        XCTAssertEqual(
            KeyErrorWords(.WrongPin(triesLeft: 1), entry: "github.com").nextStep,
            "1 try left before the key locks.")
    }

    func testAHeldPinTheKeyRefusesAsksForTheCurrentOne() {
        let words = KeyErrorWords(.HeldPinRejected(triesLeft: 1), entry: "github.com")
        XCTAssertEqual(words.problem, "Your key refused the PIN paw was holding")
        XCTAssertEqual(
            words.nextStep, "It may have been changed. Enter the current one. 1 try left before the key locks.")
    }

    func testAnotherKeyNamesTheEntryAndCountsTheKeys() {
        let one = KeyErrorWords(.NotForThisKey(recipients: ["0123456789ABCDEF"]), entry: "github.com")
        XCTAssertEqual(one.problem, "This key can't open github.com")
        XCTAssertEqual(one.nextStep, "It's encrypted for another key. Open it on a computer that has that key.")
        let two = KeyErrorWords(.NotForThisKey(recipients: ["0123456789ABCDEF", "FEDCBA9876543210"]), entry: "x")
        XCTAssertEqual(two.nextStep, "It's encrypted for other keys. Open it on a computer that has one of them.")
    }

    func testATouchTimeoutSaysToTouchWhileItBlinks() {
        XCTAssertEqual(
            KeyErrorWords(.TouchTimeout, entry: "github.com"),
            KeyErrorWords(
                problem: "Your key wasn't touched in time",
                nextStep: "Try again and touch the gold part while it blinks."))
    }

    func testCancelSaysWhatDidNotHappen() {
        XCTAssertEqual(KeyErrorWords(.Cancelled, entry: "x").nextStep, "Nothing was opened.")
        XCTAssertEqual(KeyErrorWords(.Cancelled, entry: "x", purpose: .download).nextStep, "Nothing was downloaded.")
        XCTAssertEqual(KeyErrorWords(.Cancelled, entry: "x", purpose: .send).nextStep, "Nothing was sent.")
        XCTAssertEqual(KeyErrorWords(.Cancelled, entry: "x", purpose: .check).nextStep, "Nothing was checked.")
    }

    func testThePinAskedAgainAfterTheKeyForgotItUsedNoTry() {
        XCTAssertEqual(
            KeyErrorWords(pinAskedAgain: .PinForgotten, entry: "x"),
            KeyErrorWords(problem: "Your key asked for its PIN again", nextStep: "Enter it again; no try was used."))
        // Any other reason is worded as usual.
        XCTAssertEqual(
            KeyErrorWords(pinAskedAgain: .PinFormat, entry: "x"),
            KeyErrorWords(problem: "A PIN has 6 to 127 characters", nextStep: "Enter it again; no try was used."))
    }

    func testAReaderErrorQuotesTheKey() {
        let words = KeyErrorWords(.Reader(detail: "6F00"), entry: "x")
        XCTAssertEqual(words.problem, "Your key gave an error")
        XCTAssertEqual(words.nextStep, "Unplug it, plug it back in and try again. The key said: 6F00.")
    }

    func testASyncSaysTheWaitingChangesAreKeptWhenTheKeyStops() {
        XCTAssertEqual(
            syncKeyWords(.TouchTimeout, during: .send, unpushed: 2).nextStep,
            "Try again and touch the gold part while it blinks. Your 2 changes are saved on this iPhone.")
        XCTAssertEqual(
            syncKeyWords(.KeyRemoved, during: .send, unpushed: 1).nextStep,
            "Plug it back in and try again. Your change is saved on this iPhone.")
        // A check has nothing waiting to reassure about.
        XCTAssertEqual(
            syncKeyWords(.TouchTimeout, during: .refresh, unpushed: 2).nextStep,
            "Try again and touch the gold part while it blinks.")
        XCTAssertEqual(syncKeyWords(.Cancelled, during: .send, unpushed: 2).nextStep, "Nothing was sent.")
    }
}
