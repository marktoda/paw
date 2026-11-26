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
    /// device's words. Each offers the key instead.
    static func notice(_ error: Swift.Error, entry: String) -> (problem: String, nextStep: String) {
        let paw = error as? PawError
        let nextStep = switch paw {
        case .FaceIdCancelled?, .FaceIdNotRecognised?: "Tap \(entry) to try again."
        case .FaceIdLockedOut?: "Lock and unlock your iPhone, then try again."
        default: "Open \(entry) with your key."
        }
        let said = paw.flatMap(Self.problem) ?? describe(error)
        return (Device.words(said), Device.words(nextStep))
    }
}
