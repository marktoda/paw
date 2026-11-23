import Foundation

/// Sending local changes, or checking the git remote for new ones.
enum SyncKind: Equatable {
    case send
    case refresh
}

/// Why the last send or check stopped, in words: what happened, what is
/// safe, what to do (design.md 2.1 rule 5). The core decides what went
/// wrong; this only words it.
struct SyncFailure: Equatable {
    enum Kind: Equatable {
        /// No network, or the host can't be reached. Not an error to scold.
        case offline
        case hostKeyChanged
        case keyNotAccepted
        /// A rule on the server refused the update.
        case pushRefused
        case repositoryGone
        case remoteKeptMoving
        /// The remote holds something paw will not check out.
        case refusedRemote
        /// Cancel in the key sheet. Said quietly; the pill does not change.
        case cancelled
        case other
    }

    let kind: Kind
    /// What was being done, for "Try again".
    let during: SyncKind
    let problem: String
    let nextStep: String
    /// Raw text from the host or the library, under "Details".
    var details: [String] = []
    /// A few words for the pill.
    let pill: String

    /// "Your 2 changes are still waiting on this iPhone." or, with nothing
    /// queued, "Nothing on this iPhone was changed."
    static func waiting(_ unpushed: Int) -> String {
        switch unpushed {
        case 0: "Nothing on this iPhone was changed."
        case 1: "Your change is still waiting on this iPhone."
        default: "Your \(unpushed) changes are still waiting on this iPhone."
        }
    }

    static func cancelled(during kind: SyncKind, unpushed: Int) -> SyncFailure {
        SyncFailure(
            kind: .cancelled, during: kind,
            problem: kind == .send ? "Sending was cancelled." : "Checking was cancelled.",
            nextStep: waiting(unpushed), pill: "")
    }

}

extension SyncFailure {
    /// A sync error from the core's `last_task_result`.
    init(_ error: SyncError, during kind: SyncKind, unpushed: Int, host: String) {
        let waiting = Self.waiting(unpushed)
        let notDone = kind == .send ? "Not sent" : "Not checked"
        self.during = kind
        switch error {
        case .Network(let message):
            self.kind = .offline
            problem = "\(host) can't be reached."
            nextStep = "You may be offline. \(waiting) Try again once you're connected."
            details = [message]
            pill = "Offline"
        case .HostKeyChanged(let host, let expected, let presented):
            self.kind = .hostKeyChanged
            problem = "The identity of \(host) changed."
            nextStep = "paw stopped syncing to be safe, and sent nothing. If the server's admin confirms the "
                + "change, go to Settings \u{203A} Store \u{203A} Forget server key."
            details = ["Expected \(expected.joined(separator: " or "))", "It presented \(presented)"]
            pill = "Sync stopped \u{00B7} server identity changed"
        case .KeyRejected:
            self.kind = .keyNotAccepted
            problem = "\(host) didn't accept this key."
            nextStep = "\(waiting) Check that the key's SSH key is added to your account there, then try again."
            pill = "\(notDone) \u{00B7} key not accepted"
        case .RemoteKeptMoving:
            self.kind = .remoteKeptMoving
            problem = "Your git remote kept changing while paw was sending."
            nextStep = "Nothing was lost. \(waiting) Try again in a moment."
            pill = "\(notDone) \u{00B7} try again"
        case .NotPawHistory(let commit, let reason):
            self.kind = .other
            problem = "This copy of the store has a change paw didn't make."
            nextStep = "paw won't guess what to do with it, so nothing was changed or sent."
            details = ["\(commit): \(reason)"]
            pill = "Sync stopped"
        case .UncommittedChanges(let paths):
            self.kind = .other
            problem = "The store on this iPhone has changes paw didn't make."
            nextStep = "Nothing was overwritten or sent."
            details = paths
            pill = "Sync stopped"
        case .WrongPin(let triesLeft):
            self.kind = .other
            problem = "Wrong PIN."
            nextStep = triesLeft == 1 ? "1 try left before the key locks." : "\(triesLeft) tries left. \(waiting)"
            pill = notDone
        case .PinBlocked:
            self.kind = .other
            problem = "The PIN is locked."
            nextStep = "Unlock it on a computer with your admin PIN. \(waiting)"
            pill = notDone
        case .Sign(let message):
            self.kind = .other
            problem = "Your key didn't sign the login."
            nextStep = "\(waiting) Try again, and touch the gold part of your key while it blinks."
            details = [message]
            pill = notDone
        case .Failed(let message):
            let lowered = message.lowercased()
            details = [message]
            if lowered.hasPrefix("the remote contains something paw will not check out") {
                self.kind = .refusedRemote
                problem = "Your git remote contains something paw won't check out."
                nextStep = "\(waiting) It has to be fixed from another computer."
                details = []
                pill = "Sync paused \u{00B7} Review"
            } else if lowered.contains("refused the push") {
                self.kind = .pushRefused
                problem = "Your git remote refused the changes."
                nextStep = "\(waiting) A rule on the server may not allow them; the details say why."
                pill = "Not sent \u{00B7} refused"
            } else if lowered.contains("repository not found") || lowered.contains("not found") {
                self.kind = .repositoryGone
                problem = "\(host) says the store isn't there."
                nextStep = "It may have been renamed, deleted or made private. \(waiting)"
                pill = "\(notDone) \u{00B7} store not found"
            } else {
                self.kind = .other
                problem = kind == .send ? "paw couldn't send your changes." : "paw couldn't check for changes."
                nextStep = "\(waiting) Try again; if it keeps happening, the details say why."
                pill = notDone
            }
        }
    }

    /// The step the inline row offers.
    var actionTitle: String? {
        switch kind {
        case .hostKeyChanged: "Open Settings"
        case .refusedRemote: "See what"
        case .cancelled: nil
        default: "Try again"
        }
    }

    /// Whether that step asks for the key (the disc on the button).
    var actionNeedsKey: Bool {
        actionTitle == "Try again"
    }
}

/// How the key sheet words a key failure during a sync. The words are the
/// shared ones (`KeyErrorWords`), with what is safe said for a sync.
func syncKeyWords(_ error: KeyError, during kind: SyncKind, unpushed: Int) -> KeyErrorWords {
    let words = KeyErrorWords(error, entry: "your store")
    let safe = kind == .send ? "Nothing was sent. \(SyncFailure.waiting(unpushed))" : "Nothing was changed."
    switch error {
    case .TouchTimeout:
        return KeyErrorWords(
            problem: words.problem,
            nextStep: "\(safe) Try again, and touch the gold part of your key while it blinks.")
    case .KeyRemoved:
        return KeyErrorWords(problem: words.problem, nextStep: "Plug it back in, then try again. \(safe)")
    case .Reader(let detail):
        return KeyErrorWords(
            problem: words.problem,
            nextStep: "Unplug it, plug it back in, then try again. \(safe) The key said: \(detail).")
    default:
        return words
    }
}

extension KeyErrorWords {
    init(problem: String, nextStep: String) {
        self.problem = problem
        self.nextStep = nextStep
    }
}

/// "just now", "2 min ago", "at 14:02", "on Mon", "on 3 Nov".
func syncAgo(_ date: Date, now: Date = Date()) -> String {
    let seconds = now.timeIntervalSince(date)
    if seconds < 60 { return "just now" }
    if seconds < 3_600 { return "\(Int(seconds / 60)) min ago" }
    let calendar = Calendar.current
    if calendar.isDate(date, inSameDayAs: now) {
        return "at \(date.formatted(date: .omitted, time: .shortened))"
    }
    if let days = calendar.dateComponents([.day], from: date, to: now).day, days < 7 {
        return "on \(date.formatted(.dateTime.weekday(.abbreviated)))"
    }
    return "on \(date.formatted(.dateTime.day().month(.abbreviated)))"
}
