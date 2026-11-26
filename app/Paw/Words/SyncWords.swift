import Foundation

/// Sending local changes, checking the git remote for new ones, or
/// encrypting held-back changes again for their folder's new list (no
/// network).
enum SyncKind: Equatable {
    case send
    case refresh
    case reencrypt
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
        case 1: Device.words("Your change is saved on this iPhone.")
        default: Device.words("Your \(unpushed) changes are saved on this iPhone.")
        }
    }

    /// The remote holds names paw won't check out.
    static let refusedRemoteProblem = "Your store has names paw can't download"

    static func refusedRemoteNextStep(unpushed: Int) -> String {
        sentences("Rename them on a computer.", waiting(unpushed))
    }

    /// A check with the GitHub sign-in that failed other than as a sync.
    static func signInCheckFailed(_ error: Error, unpushed: Int) -> SyncFailure {
        SyncFailure(
            kind: .other, during: .refresh, problem: "paw couldn't check for changes.",
            nextStep: waiting(unpushed), details: [describe(error)], pill: "Check failed")
    }

    static func cancelled(during kind: SyncKind, unpushed: Int) -> SyncFailure {
        let problem = switch kind {
        case .send: "Sending was cancelled."
        case .refresh: "Checking was cancelled."
        case .reencrypt: "Re-encrypting was cancelled."
        }
        return SyncFailure(kind: .cancelled, during: kind, problem: problem, nextStep: "", pill: "")
    }

}

extension SyncFailure {
    /// A sync error from a key task's `SyncFailed`, or a check with the
    /// sign-in.
    init(_ error: SyncError, during kind: SyncKind, unpushed: Int, host: String) {
        let waiting = Self.waiting(unpushed)
        let notDone = Self.notDone(kind)
        self.during = kind
        switch error {
        case .Remote(let failure):
            let remote = SyncFailure(failure, during: kind, unpushed: unpushed, host: host)
            self.kind = remote.kind
            problem = remote.problem
            nextStep = remote.nextStep
            details = remote.details
            pill = remote.pill
        case .NotPawHistory(let commit, let reason):
            self.kind = .other
            problem = Device.words("This iPhone's store has a change paw didn't make")
            nextStep = "Nothing was changed or sent."
            details = ["\(commit): \(reason)"]
            pill = "Sync stopped"
        case .UncommittedChanges(let paths):
            self.kind = .other
            problem = Device.words("This iPhone's store has changes paw didn't make")
            nextStep = "Nothing was overwritten or sent."
            details = paths
            pill = "Sync stopped"
        case .Cancelled:
            let cancelled = Self.cancelled(during: kind, unpushed: unpushed)
            self.kind = cancelled.kind
            problem = cancelled.problem
            nextStep = cancelled.nextStep
            pill = cancelled.pill
        case .StoreBusy:
            self.kind = .other
            problem = Self.couldNot(kind)
            nextStep = sentences("Try again.", waiting)
            details = [storeBusyText]
            pill = notDone
        case .Failed(let message):
            self.kind = .other
            problem = Self.couldNot(kind)
            nextStep = sentences("Try again.", waiting)
            details = [message]
            pill = notDone
        }
    }

    /// The remote's failure during a sync. `host` is the store's host.
    init(_ failure: RemoteFailure, during kind: SyncKind, unpushed: Int, host: String) {
        let waiting = Self.waiting(unpushed)
        let notDone = Self.notDone(kind)
        self.during = kind
        switch failure {
        case .noConnection(_, let message, _), .unreachable(_, let message):
            self.kind = .offline
            problem = "Can't reach \(host)"
            nextStep = sentences("Check your connection.", waiting)
            details = [message]
            pill = "Offline"
        case .hostKeyChanged(let host, let expected, let presented, _):
            self.kind = .hostKeyChanged
            problem = "\(host)'s identity changed"
            nextStep = "Sync is stopped and nothing was sent. If the server's admin confirms the change, "
                + "remove the store in Settings and set it up again."
            details = ["Expected \(expected.joined(separator: " or "))", "It presented \(presented)"]
            pill = "Sync stopped \u{00B7} Review"
        case .keyNotAccepted:
            self.kind = .keyNotAccepted
            problem = "\(host) didn't accept this key"
            nextStep = sentences("Check its SSH key is on your account there.", waiting)
            pill = "\(notDone) \u{00B7} key not accepted"
        case .keptMoving:
            self.kind = .remoteKeptMoving
            problem = "The store kept changing while paw was sending"
            nextStep = sentences("Try again in a moment.", waiting)
            pill = "\(notDone) \u{00B7} try again"
        case .refusedContent:
            self.kind = .refusedRemote
            problem = Self.refusedRemoteProblem
            nextStep = Self.refusedRemoteNextStep(unpushed: unpushed)
            pill = "Sync paused \u{00B7} Review"
        case .pushRefused(let message):
            self.kind = .pushRefused
            problem = "\(host) refused your changes"
            nextStep = sentences(waiting, "Details say why.")
            details = [message]
            pill = "Not sent \u{00B7} refused"
        case .repositoryMissing(_, _, _, let message):
            self.kind = .repositoryGone
            problem = "\(host) can't find your store"
            nextStep = sentences("It may have been renamed, deleted or made private.", waiting)
            details = [message]
            pill = "\(notDone) \u{00B7} store not found"
        default:
            self.kind = .other
            problem = Self.couldNot(kind)
            nextStep = sentences("Try again.", waiting)
            details = [remoteDetail(failure)]
            pill = notDone
        }
    }

    private static func notDone(_ kind: SyncKind) -> String {
        switch kind {
        case .send: "Not sent"
        case .refresh: "Not checked"
        case .reencrypt: "Not re-encrypted"
        }
    }

    private static func couldNot(_ kind: SyncKind) -> String {
        switch kind {
        case .send: "Couldn't send your changes"
        case .refresh: "Couldn't check for changes"
        case .reencrypt: "Couldn't re-encrypt your changes"
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

    /// Whether that step asks for the key (the disc on the button): Try
    /// again does.
    var actionNeedsKey: Bool {
        switch kind {
        case .hostKeyChanged, .refusedRemote, .cancelled: false
        default: true
        }
    }
}

/// What the key sheet's words name during a sync: the store, and that the
/// waiting changes are still here when the key stops.
func syncKeyWords(during kind: SyncKind, unpushed: Int) -> KeyErrorWords.Context {
    let purpose: KeyErrorWords.Purpose = switch kind {
    case .send: .send
    case .refresh: .check
    case .reencrypt: .open
    }
    return KeyErrorWords.Context(
        entry: "your store", purpose: purpose, waiting: kind == .refresh ? "" : SyncFailure.waiting(unpushed))
}

/// Why a held-back change can't be re-encrypted, in a few words; `nil`
/// where the held-back screen already says it.
func reencryptWords(_ reason: LeftReason) -> String? {
    switch reason {
    case .notAccepted, .noList:
        return nil
    case .missingKeys(let missing):
        return missing.count == 1
            ? "Can't re-encrypt: paw doesn't have one of the new keys."
            : "Can't re-encrypt: paw doesn't have \(missing.count) of the new keys."
    case .notForThisKey:
        return "Can't re-encrypt: your key can't open it."
    case .unreadable:
        return "Can't re-encrypt: it didn't open."
    }
}

/// "Re-encrypt 3 changes", the button and the key sheet's title.
func reencryptTitle(_ count: Int) -> String {
    "Re-encrypt \(changeCount(count))"
}

extension BackgroundRun.Outcome {
    /// How the last background check went, as the status page says it.
    var words: String {
        let words = switch self {
        case .locked: "iPhone was locked"
        case .noStore: "No store yet"
        case .notSignedIn: "Not signed in to GitHub"
        case .notLinked: "No store chosen"
        case .upToDate: "Up to date"
        case .broughtIn: "Brought in changes"
        case .paused: "Paused"
        case .signedOut: "GitHub sign-in ended"
        case .offline: "Couldn't reach GitHub"
        case .outOfTime: "Ran out of time"
        case .busy: "Store was busy"
        case .failed: "Failed"
        }
        return Device.words(words)
    }
}

extension SyncKind {
    /// "Sending 2 changes…", "Checking for changes…", "Re-encrypting…".
    func working(unpushed: Int) -> String {
        switch self {
        case .send: "Sending \(changeCount(unpushed))\u{2026}"
        case .refresh: "Checking for changes\u{2026}"
        case .reencrypt: "Re-encrypting\u{2026}"
        }
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
