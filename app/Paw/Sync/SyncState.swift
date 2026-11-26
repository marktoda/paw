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
    let pill: String
}

enum SyncRoute: Hashable {
    /// Waiting changes paw will not send as they are.
    case heldBack
    /// What the git remote holds that paw will not check out.
    case refusedRemote
    /// The git remote's history was replaced.
    case replaced
    /// "Review who can open this folder", from a held-back change or a
    /// refresh that changed a `.gpg-id`.
    case review(folder: String)
}

/// The sync sheet while it is up: the status page, or a key task's stages.
/// One sheet for both, so going from one to the other never stacks sheets.
struct SyncSheet: Identifiable {
    enum Mode: Equatable {
        case status
        /// The key sheet; its stage is `SyncModel.keyTask`'s.
        case key(kind: SyncKind)
    }

    let id: UUID
    let mode: Mode
}

enum SyncPillState: Equatable {
    case working(String)
    case upToDate(checked: Date)
    /// Never checked from this iPhone: the next check needs the key.
    case needsKey
    /// Never checked from this iPhone; the GitHub sign-in can check.
    case notChecked
    /// Changes wait here. The card under the pill counts them, so the
    /// pill says only when paw last checked.
    case notSent(checked: Date?)
    case offline(checked: Date?)
    case failed(String)
    /// Stopped until the person looks: replaced history, a refused remote
    /// or held-back changes.
    case paused(String)

    var status: SyncPill.Status {
        switch self {
        case .working: .syncing
        case .upToDate: .synced
        case .needsKey: .needsKey
        case .notChecked: .syncing
        case .notSent: .checked
        case .offline: .offline
        case .failed, .paused: .failed
        }
    }
}

/// What a sync brought in, for the dismissible card on Home.
struct SyncSummary: Identifiable, Equatable {
    enum Origin: Equatable {
        case sync
        /// After taking the git remote's replaced history.
        case replaced
    }

    let id = UUID()
    let origin: Origin
    var added: [String] = []
    var changed: [String] = []
    var removed: [String] = []
    var keptBoth: [KeptBoth] = []
    var keptLocal: [String] = []
    var droppedDeletes: [String] = []
    /// Folders whose `.gpg-id` changed ("" for the top of the store).
    var recipientsChanged: [String] = []
    /// Entries whose Face ID copy ended because their text changed.
    var endedCopies: [String] = []

    init(origin: Origin, report: IntegrateReport) {
        self.origin = origin
        keptBoth = report.keptBoth
        keptLocal = report.keptLocal
        droppedDeletes = report.droppedDeletes
        recipientsChanged = report.recipientsChanged.map { gpgIdFolder(path: $0.path) }
        // Entries the report already explains are said there instead.
        var explained = Set(keptLocal + droppedDeletes)
        for pair in keptBoth {
            explained.insert(pair.entry)
            explained.insert(pair.copy)
        }
        let sorted = { (paths: [String]) in
            paths.filter { !explained.contains($0) }.sorted { $0.localizedStandardCompare($1) == .orderedAscending }
        }
        added = sorted(report.added)
        changed = sorted(report.changed)
        removed = sorted(report.removed)
    }

    var entryCount: Int {
        added.count + changed.count + removed.count
    }

    var needsALook: Bool {
        !(keptBoth.isEmpty && keptLocal.isEmpty && droppedDeletes.isEmpty && recipientsChanged.isEmpty)
    }

    var isEmpty: Bool {
        entryCount == 0 && keptBoth.isEmpty && keptLocal.isEmpty && droppedDeletes.isEmpty
            && recipientsChanged.isEmpty
    }
}
