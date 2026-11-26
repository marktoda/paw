import Foundation

/// What the key sheet says about a `KeyError`, and about a key that could
/// not be read: what happened, then what to do next. Every workflow that
/// uses the key (opening, AutoFill, setup, sync, the touch check) words
/// them here (design.md 2.3 and 2.11); what differs between workflows is a
/// `Context` value, never a sentence of its own.
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

    /// What changes the words between workflows.
    struct Context {
        /// What the key was opening: an entry's name, "your store".
        var entry: String
        var purpose: Purpose = .open
        /// Said after the key stopped (unplugged, not touched) when work is
        /// still waiting, such as a sync's unsent changes; "" for none.
        var waiting: String = ""
        var table: WordTable = .app

        /// `error` in this workflow's words; `pinAskedAgain` when the PIN
        /// stage is shown again because of it.
        func words(_ error: KeyError, pinAskedAgain: Bool = false) -> KeyErrorWords {
            pinAskedAgain
                ? KeyErrorWords(pinAskedAgain: error, entry: entry, purpose: purpose, waiting: waiting, table: table)
                : KeyErrorWords(error, entry: entry, purpose: purpose, waiting: waiting, table: table)
        }
    }

    let problem: String
    let nextStep: String

    /// The key is locked after too many wrong PINs: the sheet's own stage,
    /// and the key's answer wherever else it comes.
    static let locked = KeyErrorWords(
        problem: "Your key is locked",
        nextStep: "Unlock it on a computer with its reset code or admin PIN. "
            + "Passwords that open with Face ID still work.")

    init(problem: String, nextStep: String) {
        self.problem = problem
        self.nextStep = nextStep
    }

    /// What the PIN stage says when it is asked again because of `error`
    /// (any but a wrong PIN, which has its own stage). `PinForgotten` there
    /// means the key lost the PIN's verification during the task: paw
    /// dropped the PIN without trying it again, so no try was used.
    init(
        pinAskedAgain error: KeyError, entry: String, purpose: Purpose = .open, waiting: String = "",
        table: WordTable
    ) {
        if case .PinForgotten = error {
            self.init(problem: "Your key asked for its PIN again", nextStep: "Enter it again; no try was used.")
        } else {
            self.init(error, entry: entry, purpose: purpose, waiting: waiting, table: table)
        }
    }

    init(_ error: KeyError, entry: String, purpose: Purpose = .open, waiting: String = "", table: WordTable) {
        switch error {
        case .WrongPin(let triesLeft):
            problem = "Wrong PIN"
            nextStep = KeySheetWords.triesLeft(Int(triesLeft), afterWrongPin: false)
        case .HeldPinRejected(let triesLeft):
            problem = "Your key refused the PIN paw was holding"
            nextStep = "It may have been changed. Enter the current one. "
                + KeySheetWords.triesLeft(Int(triesLeft), afterWrongPin: false)
        case .PinBlocked:
            self = .locked
        case .PinFormat:
            problem = "A PIN has 6 to 127 characters"
            nextStep = "Enter it again; no try was used."
        case .PinForgotten:
            problem = "paw no longer has your PIN"
            nextStep = "Try again and enter it."
        case .TouchTimeout:
            problem = "Your key wasn't touched in time"
            nextStep = sentences("Try again and touch the gold part while it blinks.", waiting)
        case .KeyRemoved:
            problem = "Your key was unplugged"
            nextStep = sentences("Plug it back in and try again.", waiting)
        case .NotForThisKey(let recipients):
            problem = "This key can't open \(entry)"
            nextStep = recipients.count == 1
                ? "It's encrypted for another key. Open it on a computer that has that key."
                : "It's encrypted for other keys. Open it on a computer that has one of them."
        case .DifferentKey(let mismatch):
            self = table.differentKey(mismatch)
        case .NoDecryptionKey:
            problem = "This key has no decryption key"
            nextStep = "paw needs a key with an OpenPGP decryption key. Try another key."
        case .NoAuthenticationKey:
            problem = "This key has no SSH key"
            nextStep = "Add an authentication subkey on a computer, then try again. It can still open passwords."
        case .AuthenticationKeyNotEd25519(let algorithm):
            problem = "This key has no SSH key"
            nextStep = "Its authentication key is \(algorithm). paw needs an Ed25519 key."
        case .EntryUnreadable(let detail):
            problem = "\(entry) can't be read"
            nextStep = "Its file may be damaged (\(detail))."
        case .StoreBusy:
            problem = StoreBusyWords.problem
            nextStep = StoreBusyWords.nextStep
        case .DecryptFailed:
            problem = "This key couldn't open \(entry)"
            nextStep = "It may be encrypted for another key."
        case .Reader(let detail), .Internal(let detail):
            problem = "Your key gave an error"
            nextStep = "Unplug it, plug it back in and try again. The key said: \(detail)."
        case .Cancelled:
            problem = "Cancelled"
            nextStep = purpose.nothingHappened
        }
    }

    /// "paw couldn't read your key", then why and what to do: the Plug in
    /// stage after a failed read.
    init(_ problem: KeyReadProblem, table: WordTable) {
        switch problem {
        case .unsupported:
            self = KeyErrorWords(
                problem: "This iPhone can't use USB keys",
                nextStep: "paw needs an iPhone with USB-C and smart card support.")
        case .failed(let error):
            self = table.keyUnreadable(error)
        }
    }

    /// "serial 12 345 678, ending 2C11 41DE".
    static func spoken(_ key: KeyIdentity) -> String {
        var words = "serial \(spokenSerial(key.ident))"
        if let fingerprint = key.decryptionFingerprint {
            words += ", ending \(fingerprintEnding(fingerprint))"
        }
        return words
    }
}
