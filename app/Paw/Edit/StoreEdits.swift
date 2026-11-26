import Foundation
import Observation

/// What the add and edit sheet is for.
enum EntryFormPurpose: Identifiable, Equatable {
    /// A new entry, starting in this folder ("" for the top).
    case add(folder: String)
    /// The open entry shown under this session id. `focusName` starts on
    /// the name, for "Rename or move".
    case edit(sessionID: UUID, focusName: Bool)

    var id: String {
        switch self {
        case .add(let folder): "add-\(folder)"
        case .edit(let session, _): "edit-\(session)"
        }
    }
}

/// "Deleted github.com. Undo": the change Undo takes back, if it is still
/// the newest one when tapped.
struct UndoOffer: Identifiable, Equatable {
    let id = UUID()
    let message: String
    let entryName: String
    /// The unpushed change's id (`QueueItem.id`).
    let change: String
}

/// The write side of the store as the screens use it: the core's writer,
/// the unpushed queue, and what the edit screens show now. Saving,
/// deleting and undoing need no key and no network. Each write is
/// announced on the live store, which every reader of the store follows.
@MainActor @Observable
final class StoreEdits {
    let writer: StoreWriter
    /// The queue lives on the sync object; `nil` if it could not be opened,
    /// and then the unpushed list says so.
    let sync: StoreSync?
    /// The core's state directory of the store.
    let stateDir: String
    @ObservationIgnored private let live: LiveStore

    /// Newest first.
    private(set) var unpushed: [QueueItem] = []
    /// Why the queue could not be read or changed, said on the list.
    var queueProblem: String?
    /// The add or edit sheet, while it is up.
    var form: EntryFormPurpose?
    /// The open entry whose delete is waiting for its inline confirm.
    var confirmingDelete: UUID?
    var undoOffer: UndoOffer?

    init(writer: StoreWriter, sync: StoreSync?, stateDir: String, live: LiveStore) {
        self.writer = writer
        self.sync = sync
        self.stateDir = stateDir
        self.live = live
        refreshQueue()
        live.changed.subscribe(self) { edits, changed in
            if changed.change.rewritesEntries {
                edits.refreshQueue()
            }
        }
    }

    func refreshQueue() {
        guard let sync else {
            unpushed = []
            return
        }
        do {
            unpushed = try sync.unpushed()
            queueProblem = nil
        } catch {
            queueProblem = describe(error)
        }
    }

    /// "3 changes not sent".
    var bannerTitle: String {
        "\(changeCount(unpushed.count)) not sent"
    }

    // MARK: Writes

    /// Saves the form's entry; `nil` if the form refused or said why not.
    func save(_ form: EntryFormModel, copies: CopyCache?) -> Saved? {
        guard let saved = form.save(copies: copies) else { return nil }
        let renamed = saved.previous.map { StoreChange.Renamed(from: $0, to: saved.entry) }
        live.announce(.wrote(entries: [saved.previous, saved.entry].compactMap { $0 }, renamed: renamed))
        return saved
    }

    func delete(_ entry: String, copies: CopyCache?) throws {
        _ = try BackgroundTime.run("Delete") { try writer.delete(entry: entry, copies: copies) }
        live.announce(.wrote(entries: [entry], renamed: nil))
    }

    /// Takes back the newest change. Nothing without the queue. `task`
    /// names the time asked of iOS to finish it.
    func undoLast(task: String = "Undo") throws {
        guard let sync else { return }
        let undone = try BackgroundTime.run(task) { try sync.undoLast() }
        announce(undone)
    }

    func discard(_ id: String) throws {
        guard let sync else { return }
        let discarded = try BackgroundTime.run("Change") { try sync.discard(id: id) }
        announce(discarded)
    }

    func discardAll() throws {
        guard let sync else { return }
        let entries = unpushed.map(\.entry)
        try BackgroundTime.run("Change") { try sync.discardAll() }
        live.announce(.wrote(entries: entries, renamed: nil))
    }

    /// "Accept the new list" for `folder`.
    func acceptRecipients(folder: String) throws {
        try writer.acceptRecipients(folder: folder)
        live.announce(.recipientsAccepted(folder: folder))
    }

    private func announce(_ changes: [QueueItem]) {
        live.announce(.wrote(entries: changes.flatMap { [$0.previous, $0.entry].compactMap { $0 } }, renamed: nil))
    }
}
