import Foundation

/// The sentences that differ by where paw speaks. The app has room to say
/// what happened and why; AutoFill, a sheet over another app's form, says
/// what to do. A workflow picks its table when it is made; every other
/// sentence is the same in both.
struct WordTable: Sendable {
    /// The key plugged in is not the store's.
    let differentKey: @Sendable (KeyMismatch) -> KeyErrorWords
    /// The key is in, but reading it failed.
    let keyUnreadable: @Sendable (Swift.Error) -> KeyErrorWords
    /// Face ID did not open the entry named.
    let faceIdNotice: @Sendable (Swift.Error, _ entry: String) -> FaceIdNotice

    static let app = WordTable(
        differentKey: { mismatch in
            KeyErrorWords(
                problem: "This isn't the key this store was set up with",
                nextStep: "This key: \(KeyErrorWords.spoken(mismatch.presented)). "
                    + "The store's key: \(KeyErrorWords.spoken(mismatch.enrolled)). "
                    + "Plug that one in and try again. No PIN try was used.")
        },
        keyUnreadable: { error in
            let why: String
            switch error as? KeyError {
            case .KeyRemoved?:
                why = "It stopped answering."
            case .Reader(let detail)?:
                why = "The key said: \(detail)."
            case let error?:
                why = KeyErrorWords(error, entry: "this password").problem + "."
            case nil:
                if case .Failed(let message)? = error as? ConnectionError {
                    why = message
                } else {
                    why = describe(error)
                }
            }
            return KeyErrorWords(
                problem: "paw couldn't read your key",
                nextStep: "\(why) Try again, or unplug the key and plug it back in.")
        },
        faceIdNotice: { error, entry in
            let paw = error as? PawError
            let nextStep = switch paw {
            case .FaceIdCancelled?, .FaceIdNotRecognised?: "Tap \(entry) to try again."
            case .FaceIdLockedOut?: "Lock and unlock your iPhone, then try again."
            default: "Open \(entry) with your key."
            }
            let said = paw.flatMap(FaceIdWords.problem) ?? describe(error)
            return FaceIdNotice(problem: Device.words(said), nextStep: Device.words(nextStep), offersKey: true)
        })

    static let autoFill = WordTable(
        differentKey: { _ in
            KeyErrorWords(problem: "This isn't your store's key", nextStep: "Plug in the key you set paw up with.")
        },
        keyUnreadable: { _ in
            KeyErrorWords(problem: "paw couldn't read your key", nextStep: "Unplug it, plug it back in, then try again.")
        },
        faceIdNotice: { error, entry in
            if isStoreBusy(error) {
                return FaceIdNotice(problem: "The store is busy", nextStep: "Tap \(entry) again.", offersKey: false)
            }
            let short: (problem: String, nextStep: String)? = switch error as? PawError {
            case .FaceIdLockedOut?: ("Face ID is locked", "Lock and unlock your iPhone, then try again.")
            case .FaceIdChanged?: ("Face ID changed on this iPhone", "")
            case .FaceIdCancelled?, .FaceIdNotRecognised?, .FaceIdNotSetUp?: nil
            default: ("Face ID didn't open \(entry)", "")
            }
            guard let short else { return WordTable.app.faceIdNotice(error, entry) }
            return FaceIdNotice(problem: Device.words(short.problem), nextStep: Device.words(short.nextStep), offersKey: true)
        })
}

/// What a Face ID open that failed says above the list it was tapped in.
struct FaceIdNotice: Equatable {
    let problem: String
    let nextStep: String
    /// Offers to open the entry with the key instead.
    let offersKey: Bool
}

/// AutoFill's own problems: a store it cannot read, an entry with nothing
/// of the kind asked for.
enum AutoFillWords {
    /// The store is there but could not be read, or the shared folder
    /// cannot be reached.
    static let storeUnreadable = (problem: "paw couldn't read your store", nextStep: "Open paw to fix it.")

    /// The entry has no password, or no one-time code.
    static func nothingToFill(_ entry: String, _ kind: AutoFillKind, fromList: Bool)
        -> (problem: String, nextStep: String) {
        ("\(entry) has no \(kind.missing)", fromList ? "Pick another entry." : "Add one in paw, then try again.")
    }
}
