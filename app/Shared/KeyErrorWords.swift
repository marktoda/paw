import Foundation

/// What the key sheet says about a `KeyError`: what happened, then what to
/// do next. Every case is worded here and nowhere else (design.md 2.3 and
/// 2.11). `entry` is the name of the entry being opened; `purpose` says
/// what the key was for, so "nothing was opened" fits a download too.
struct KeyErrorWords: Equatable {
    /// What the key was being used for.
    enum Purpose {
        case open, download, send, check

        /// "Nothing was opened.", by what did not happen. Said after Cancel.
        var nothingHappened: String {
            switch self {
            case .open: "Nothing was opened."
            case .download: "Nothing was downloaded."
            case .send: "Nothing was sent."
            case .check: "Nothing was checked."
            }
        }
    }

    let problem: String
    let nextStep: String

    init(problem: String, nextStep: String) {
        self.problem = problem
        self.nextStep = nextStep
    }

    /// What the PIN stage says when it is asked again because of `error`
    /// (any but a wrong PIN, which has its own stage). `PinForgotten` there
    /// means the key lost the PIN's verification during the task: paw
    /// dropped the PIN without trying it again, so no try was used.
    init(pinAskedAgain error: KeyError, entry: String, purpose: Purpose = .open) {
        if case .PinForgotten = error {
            self.init(problem: "Your key asked for its PIN again", nextStep: "Enter it again; no try was used.")
        } else {
            self.init(error, entry: entry, purpose: purpose)
        }
    }

    init(_ error: KeyError, entry: String, purpose: Purpose = .open) {
        switch error {
        case .WrongPin(let triesLeft):
            problem = "Wrong PIN"
            nextStep = triesLeft == 1 ? "1 try left before the key locks." : "\(triesLeft) tries left."
        case .HeldPinRejected(let triesLeft):
            problem = "Your key refused the PIN paw was holding"
            nextStep = triesLeft == 1
                ? "It may have been changed. Enter the current one. 1 try left before the key locks."
                : "It may have been changed. Enter the current one. \(triesLeft) tries left."
        case .PinBlocked:
            problem = "Your key is locked"
            nextStep = "Unlock it on a computer with its reset code or admin PIN. "
                + "Passwords that open with Face ID still work."
        case .PinFormat:
            problem = "A PIN has 6 to 127 characters"
            nextStep = "Enter it again; no try was used."
        case .PinForgotten:
            problem = "paw no longer has your PIN"
            nextStep = "Try again and enter it."
        case .TouchTimeout:
            problem = "Your key wasn't touched in time"
            nextStep = "Try again and touch the gold part while it blinks."
        case .KeyRemoved:
            problem = "Your key was unplugged"
            nextStep = "Plug it back in and try again."
        case .NotForThisKey(let recipients):
            problem = "This key can't open \(entry)"
            nextStep = recipients.count == 1
                ? "It's encrypted for another key. Open it on a computer that has that key."
                : "It's encrypted for other keys. Open it on a computer that has one of them."
        case .DifferentKey:
            problem = "This key can't open \(entry)"
            nextStep = "It's encrypted for another key. Open it on a computer that has that key."
        case .NoDecryptionKey:
            problem = "This key has no decryption key"
            nextStep = "paw needs a key with an OpenPGP decryption key. Try another key."
        case .NoAuthenticationKey:
            problem = "This key has no SSH key"
            nextStep = "Add an authentication subkey on a computer, then try again. It can still open passwords."
        case .EntryUnreadable(let detail):
            problem = "\(entry) can't be read"
            nextStep = "Its file may be damaged (\(detail))."
        case .StoreBusy:
            problem = "\(entry) can't be read"
            nextStep = "Its file may be damaged (\(storeBusyText))."
        case .DecryptFailed:
            problem = "This key couldn't open \(entry)"
            nextStep = "It may be encrypted for another key."
        case .Reader(let detail):
            problem = "Your key gave an error"
            nextStep = "Unplug it, plug it back in and try again. The key said: \(detail)."
        case .Cancelled:
            problem = "Cancelled"
            nextStep = purpose.nothingHappened
        }
    }
}
