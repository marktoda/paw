import Foundation

/// What the key sheet says, by stage: for the sheet itself and for the
/// touch check in Key diagnostics, which shows a task's stages inline.
enum KeySheetWords {
    /// Fewer PIN tries than this are said before any input.
    static let warnsBelowTries = 3

    /// "2 tries left.", or after a wrong PIN "2 tries left before the key
    /// locks."; the last try always says the key locks.
    static func triesLeft(_ triesLeft: Int, afterWrongPin: Bool) -> String {
        if triesLeft == 1 {
            return "1 try left before the key locks."
        }
        return afterWrongPin ? "\(triesLeft) tries left before the key locks." : "\(triesLeft) tries left."
    }

    /// The Working stage of an open: "Opening github.com…".
    static func opening(_ entry: String) -> String {
        "Opening \(entry)\u{2026}"
    }
}

extension KeySheetStage {
    var headline: String {
        switch self {
        case .plugIn: "Plug in your key"
        case .pin: "Enter your key's PIN"
        case .wrongPin: "Wrong PIN"
        case .pinAgain(_, let problem, _): problem
        case .busy: "One moment"
        case .touch: "Touch your key"
        case .working(let message, _): message
        case .done: "Done"
        case .locked: KeyErrorWords.locked.problem
        case .failed(let problem, _): problem
        }
    }

    /// The line under the headline, if the stage has one.
    var explanation: String? {
        switch self {
        case .plugIn:
            return nil
        case .pin(_, let forgotten):
            return forgotten
                ? "Asked again after 15 minutes away."
                : "Not your iPhone passcode."
        case .wrongPin(let triesLeft):
            return triesLeft == 1 ? "Not sure? Stop and check your PIN first." : nil
        case .pinAgain(_, _, let nextStep):
            return nextStep
        case .busy:
            return "Your key is finishing something else."
        case .touch:
            return "The gold part, while it blinks."
        case .working(_, let progress):
            return progress
        case .done:
            return nil
        case .locked:
            return KeyErrorWords.locked.nextStep
        case .failed(_, let nextStep):
            return nextStep
        }
    }

    /// The PIN tries the key has left, on the PIN stages.
    var triesLeft: Int? {
        switch self {
        case .pin(let triesLeft, _), .wrongPin(let triesLeft), .pinAgain(let triesLeft, _, _): triesLeft
        default: nil
        }
    }

    /// The PIN stage's button.
    var continueTitle: String {
        triesLeft == 1 ? "Use last try" : "Continue"
    }

    /// The PIN was just refused: a wrong PIN, or one asked again for a
    /// reason.
    var followsRefusedPin: Bool {
        switch self {
        case .wrongPin, .pinAgain: true
        default: false
        }
    }

    /// How many PIN tries are left, said before any input whenever fewer
    /// than 3 remain.
    var triesLine: String? {
        guard let triesLeft, triesLeft < KeySheetWords.warnsBelowTries else { return nil }
        return KeySheetWords.triesLeft(triesLeft, afterWrongPin: followsRefusedPin)
    }
}

extension TaskProgress {
    /// How far a task has got, under the Working stage: "412 of 1,240".
    var counted: String {
        "\(Int(done).formatted()) of \(Int(total).formatted())"
    }
}
