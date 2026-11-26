import Foundation

/// What a Face ID open that failed says, in the app and in AutoFill.
enum FaceIdWords {
    /// What happened, in a few words; `nil` for an error that is not Face
    /// ID's.
    static func problem(_ error: PawError) -> String? {
        switch error {
        case .FaceIdCancelled: "Face ID was cancelled"
        case .FaceIdNotRecognised: "Face ID didn't recognise you"
        case .FaceIdLockedOut: "Face ID is off until you enter your iPhone passcode"
        case .FaceIdNotSetUp: "Face ID isn't set up on this iPhone"
        case .FaceIdChanged: "Face ID changed, so its saved copies were removed"
        default: nil
        }
    }

    /// The notice after tapping `entry` failed with `error`, in this
    /// device's words, and whether it offers the key instead.
    static func notice(_ error: Swift.Error, entry: String, shownIn: ShownIn = .app)
        -> (problem: String, nextStep: String, offersKey: Bool) {
        let paw = error as? PawError
        if shownIn == .autoFill, isStoreBusy(error) {
            return ("The store is busy", "Tap \(entry) again.", false)
        }
        if shownIn == .autoFill, let short = autoFillNotice(paw, entry: entry) {
            return (Device.words(short.problem), Device.words(short.nextStep), true)
        }
        let nextStep = switch paw {
        case .FaceIdCancelled?, .FaceIdNotRecognised?: "Tap \(entry) to try again."
        case .FaceIdLockedOut?: "Lock and unlock your iPhone, then try again."
        default: "Open \(entry) with your key."
        }
        let said = paw.flatMap(Self.problem) ?? describe(error)
        return (Device.words(said), Device.words(nextStep), true)
    }

    /// AutoFill's shorter notices; `nil` where it says what the app says.
    private static func autoFillNotice(_ error: PawError?, entry: String) -> (problem: String, nextStep: String)? {
        switch error {
        case .FaceIdLockedOut?:
            return ("Face ID is locked", "Lock and unlock your iPhone, then try again.")
        case .FaceIdChanged?:
            return ("Face ID changed on this iPhone", "")
        case .FaceIdCancelled?, .FaceIdNotRecognised?, .FaceIdNotSetUp?:
            return nil
        default:
            return ("Face ID didn't open \(entry)", "")
        }
    }
}
