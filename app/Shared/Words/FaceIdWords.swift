import Foundation

/// What Face ID says: a failed open, and how long a copy lasts.
enum FaceIdWords {
    /// What to do while Face ID is locked out.
    static let lockedOutNextStep = "Lock and unlock your iPhone, then try again."

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

    /// The notice after tapping `entry` failed with `error`. A busy store asks for another tap, never the key:
    /// the key would wait for the same store.
    static func notice(_ error: Swift.Error, entry: EntryRef, table: WordTable) -> Notice {
        if isStoreBusy(error) {
            return Notice(problem: StoreBusyWords.problem, nextStep: StoreBusyWords.nextStep)
        }
        return table.faceIdNotice(error, entry)
    }

    /// How long a Face ID copy lasts: "8 hours".
    static var window: String {
        "\(FaceIdOpener.windowSecs / 3600) hours"
    }

    /// The key sheet's note when a copy will be kept, by what the copy
    /// does: "Then it opens with Face ID for 8 hours."
    static func afterKey(_ verb: String) -> String {
        "Then it \(verb) with Face ID for \(window)."
    }
}
