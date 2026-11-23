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

    /// "Your 2 changes are saved on this iPhone." Nothing when none wait:
    /// there is then nothing to reassure about.
    static func waiting(_ unpushed: Int) -> String {
        switch unpushed {
        case 0: ""
        case 1: "Your change is saved on this iPhone."
        default: "Your \(unpushed) changes are saved on this iPhone."
        }
    }

    /// Sentences in a row, leaving out the empty ones.
    static func sentences(_ parts: String...) -> String {
        parts.filter { !$0.isEmpty }.joined(separator: " ")
    }

    static func cancelled(during kind: SyncKind, unpushed: Int) -> SyncFailure {
        SyncFailure(
            kind: .cancelled, during: kind,
            problem: kind == .send ? "Sending was cancelled." : "Checking was cancelled.",
            nextStep: "", pill: "")
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
            problem = "Can't reach \(host)"
            nextStep = Self.sentences("Check your connection.", waiting)
            details = [message]
            pill = "Offline"
        case .HostKeyChanged(let host, let expected, let presented):
            self.kind = .hostKeyChanged
            problem = "\(host)'s identity changed"
            nextStep = "Sync is stopped and nothing was sent. If the server's admin confirms the change, "
                + "remove the store in Settings and set it up again."
            details = ["Expected \(expected.joined(separator: " or "))", "It presented \(presented)"]
            pill = "Sync stopped \u{00B7} Review"
        case .KeyRejected:
            self.kind = .keyNotAccepted
            problem = "\(host) didn't accept this key"
            nextStep = Self.sentences("Check its SSH key is on your account there.", waiting)
            pill = "\(notDone) \u{00B7} key not accepted"
        case .RemoteKeptMoving:
            self.kind = .remoteKeptMoving
            problem = "The store kept changing while paw was sending"
            nextStep = Self.sentences("Try again in a moment.", waiting)
            pill = "\(notDone) \u{00B7} try again"
        case .NotPawHistory(let commit, let reason):
            self.kind = .other
            problem = "This iPhone's store has a change paw didn't make"
            nextStep = "Nothing was changed or sent."
            details = ["\(commit): \(reason)"]
            pill = "Sync stopped"
        case .UncommittedChanges(let paths):
            self.kind = .other
            problem = "This iPhone's store has changes paw didn't make"
            nextStep = "Nothing was overwritten or sent."
            details = paths
            pill = "Sync stopped"
        case .WrongPin(let triesLeft):
            self.kind = .other
            problem = "Wrong PIN"
            nextStep = triesLeft == 1 ? "1 try left before the key locks." : "\(triesLeft) tries left."
            pill = notDone
        case .PinBlocked:
            self.kind = .other
            problem = "Your key is locked"
            nextStep = "Unlock it on a computer with its reset code or admin PIN."
            pill = notDone
        case .Sign(let message):
            self.kind = .other
            problem = "Your key didn't sign in"
            nextStep = "Try again and touch the gold part while it blinks."
            details = [message]
            pill = notDone
        case .Failed(let message):
            let lowered = message.lowercased()
            details = [message]
            if lowered.hasPrefix("the remote contains something paw will not check out") {
                self.kind = .refusedRemote
                problem = "Your store has names paw can't download"
                nextStep = Self.sentences("Rename them on a computer.", waiting)
                details = []
                pill = "Sync paused \u{00B7} Review"
            } else if lowered.contains("refused the push") {
                self.kind = .pushRefused
                problem = "\(host) refused your changes"
                nextStep = Self.sentences(waiting, "Details say why.")
                pill = "Not sent \u{00B7} refused"
            } else if lowered.contains("repository not found") || lowered.contains("not found") {
                self.kind = .repositoryGone
                problem = "\(host) can't find your store"
                nextStep = Self.sentences("It may have been renamed, deleted or made private.", waiting)
                pill = "\(notDone) \u{00B7} store not found"
            } else {
                self.kind = .other
                problem = kind == .send ? "Couldn't send your changes" : "Couldn't check for changes"
                nextStep = Self.sentences("Try again.", waiting)
                pill = notDone
            }
        }
    }

    /// The step the inline row offers.
    var actionTitle: String? {
        switch kind {
        case .hostKeyChanged: "Open Settings"
        case .refusedRemote: "Show names"
        case .cancelled: nil
        default: "Try again"
        }
    }

    /// Whether that step asks for the key (the disc on the button).
    var actionNeedsKey: Bool {
        actionTitle == "Try again"
    }
}

/// How the key sheet words a key failure during a sync: the shared words
/// (`KeyErrorWords`), and that the waiting changes are still here.
func syncKeyWords(_ error: KeyError, during kind: SyncKind, unpushed: Int) -> KeyErrorWords {
    let words = KeyErrorWords(error, entry: "your store", purpose: kind == .send ? .send : .check)
    switch error {
    case .TouchTimeout, .KeyRemoved:
        return KeyErrorWords(
            problem: words.problem,
            nextStep: SyncFailure.sentences(words.nextStep, kind == .send ? SyncFailure.waiting(unpushed) : ""))
    default:
        return words
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
