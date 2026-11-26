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
            syncKeyWords(during: .send, unpushed: 2).words(.TouchTimeout).nextStep,
            "Try again and touch the gold part while it blinks. Your 2 changes are saved on this iPhone.")
        XCTAssertEqual(
            syncKeyWords(during: .send, unpushed: 1).words(.KeyRemoved).nextStep,
            "Plug it back in and try again. Your change is saved on this iPhone.")
        // A check has nothing waiting to reassure about.
        XCTAssertEqual(
            syncKeyWords(during: .refresh, unpushed: 2).words(.TouchTimeout).nextStep,
            "Try again and touch the gold part while it blinks.")
        XCTAssertEqual(syncKeyWords(during: .send, unpushed: 2).words(.Cancelled).nextStep, "Nothing was sent.")
    }
}

/// Words every workflow shares: the key that is not the store's, a key
/// that can't be read, and a Face ID open that failed.
final class SharedWordsTests: XCTestCase {
    func testAnotherKeyThanTheStoresNamesBothKeys() {
        let presented = KeyIdentity(ident: "0006:12345678", decryptionFingerprint: "AAAA0000BBBB1111CCCC2222DDDD3333EEEE4444", authenticationFingerprint: nil)
        let enrolled = KeyIdentity(ident: "0006:87654321", decryptionFingerprint: nil, authenticationFingerprint: nil)
        let words = KeyErrorWords(.DifferentKey(mismatch: KeyMismatch(enrolled: enrolled, presented: presented)), entry: "x")
        XCTAssertEqual(words.problem, "This isn't the key this store was set up with")
        XCTAssertEqual(
            words.nextStep,
            "This key: serial 12 345 678, ending EEEE 4444. The store's key: serial 87 654 321. "
                + "Plug that one in and try again. No PIN try was used.")
    }

    func testAKeyThatCannotBeReadSaysWhyAndWhatToDo() {
        XCTAssertEqual(
            KeyErrorWords(.failed(KeyError.KeyRemoved)),
            KeyErrorWords(
                problem: "paw couldn't read your key",
                nextStep: "It stopped answering. Try again, or unplug the key and plug it back in."))
        XCTAssertEqual(KeyErrorWords(.unsupported).problem, "This iPhone can't use USB keys")
    }

    func testFaceIdNoticesOfferTheKey() {
        let locked = FaceIdWords.notice(PawError.FaceIdLockedOut, entry: "github.com")
        XCTAssertEqual(locked.problem, "Face ID is off until you enter your iPhone passcode")
        XCTAssertEqual(locked.nextStep, "Lock and unlock your iPhone, then try again.")
        let changed = FaceIdWords.notice(PawError.FaceIdChanged, entry: "github.com")
        XCTAssertEqual(changed.problem, "Face ID changed, so its saved copies were removed")
        XCTAssertEqual(changed.nextStep, "Open github.com with your key.")
        XCTAssertEqual(describe(PawError.FaceIdNotRecognised), "Face ID didn't recognise you.")
    }

    func testAutoFillSaysItMoreShortly() {
        let autoFill = KeyErrorWords.Context(entry: "x", shownIn: .autoFill)
        let presented = KeyIdentity(ident: "0006:1", decryptionFingerprint: nil, authenticationFingerprint: nil)
        XCTAssertEqual(
            autoFill.words(.DifferentKey(mismatch: KeyMismatch(enrolled: presented, presented: presented))),
            KeyErrorWords(problem: "This isn't your store's key", nextStep: "Plug in the key you set paw up with."))
        XCTAssertEqual(
            KeyErrorWords(.failed(KeyError.KeyRemoved), shownIn: .autoFill).nextStep,
            "Unplug it, plug it back in, then try again.")
        let locked = FaceIdWords.notice(PawError.FaceIdLockedOut, entry: "github.com", shownIn: .autoFill)
        XCTAssertEqual(locked.problem, "Face ID is locked")
        let other = FaceIdWords.notice(PawError.CopyUnavailable, entry: "github.com", shownIn: .autoFill)
        XCTAssertEqual(other.problem, "Face ID didn't open github.com")
        XCTAssertEqual(other.nextStep, "")
        XCTAssertFalse(FaceIdWords.notice(PawError.StoreBusy, entry: "github.com", shownIn: .autoFill).offersKey)
    }
}
