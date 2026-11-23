import Foundation

/// What the key sheet says about a `KeyError`: what happened, then what to
/// do next. Every case is worded here and nowhere else (design.md 2.3 and
/// 2.11). `entry` is the name of the entry being opened.
struct KeyErrorWords: Equatable {
    let problem: String
    let nextStep: String

    init(_ error: KeyError, entry: String) {
        switch error {
        case .WrongPin(let triesLeft):
            problem = "Wrong PIN"
            nextStep = triesLeft == 1
                ? "1 try left before the key locks. If you're unsure, stop and check your PIN first."
                : "\(triesLeft) tries left. Enter the PIN for the key, not your iPhone passcode."
        case .HeldPinRejected(let triesLeft):
            problem = "Your key didn't take the PIN paw had"
            let tries = triesLeft == 1 ? "1 try left before the key locks" : "\(triesLeft) tries left"
            nextStep = "It may have been changed on another computer. Enter the PIN again (\(tries))."
        case .PinBlocked:
            problem = "The PIN is locked"
            nextStep = "Three wrong PINs lock the key. Unlock it on a computer with your admin PIN. "
                + "Passwords that open with Face ID still work."
        case .PinFormat:
            problem = "A PIN has 6 to 127 characters"
            nextStep = "Nothing was sent to your key, so no try was used. Check the PIN and enter it again."
        case .PinForgotten:
            problem = "paw forgot your PIN"
            nextStep = "It was forgotten before your key was used. Try again and enter it."
        case .TouchTimeout:
            problem = "Your key wasn't touched in time"
            nextStep = "Nothing was opened. Try again, and touch the gold part of your key while it blinks."
        case .KeyRemoved:
            problem = "Your key was unplugged"
            nextStep = "Plug it back in, then try again. paw will ask for your PIN again."
        case .NotForThisKey(let recipients):
            problem = "This key can't open \(entry)"
            nextStep = recipients.count == 1
                ? "It's encrypted for another key. Open it on a computer that has that key."
                : "It's encrypted for other keys. Open it on a computer that has one of them."
        case .NoDecryptionKey:
            problem = "This key has no decryption key"
            nextStep = "paw needs a YubiKey with an OpenPGP decryption key. Plug in another key, then try again."
        case .NoAuthenticationKey:
            problem = "This key has no SSH key"
            nextStep = "Add an authentication subkey to this key on a computer, then try again. "
                + "It can still open passwords."
        case .EntryUnreadable(let detail):
            problem = "\(entry) isn't a readable password"
            nextStep = "The file may be damaged (\(detail)). Other passwords are not affected."
        case .DecryptFailed:
            problem = "This key couldn't open \(entry)"
            nextStep = "It may be encrypted for another key. Nothing was kept. Try again with that key."
        case .Reader(let detail):
            problem = "Your key answered unexpectedly"
            nextStep = "Unplug it, plug it back in, then try again. The key said: \(detail)."
        case .Cancelled:
            problem = "Cancelled"
            nextStep = "Nothing was opened."
        }
    }
}
