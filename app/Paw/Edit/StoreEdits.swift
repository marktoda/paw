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

/// Adding, editing, deleting and undoing: the form, the delete confirm and
/// the Undo offer, and every write behind them. None needs the key or the
/// network: encryption uses the public keys pinned at setup, and each
/// change is one local commit. Each write is announced on the live store;
/// what the screens should show next is asked through `requests`.
@MainActor @Observable
final class StoreEdits {
    let writer: StoreWriter
    /// The unpushed changes, from sync; `nil` if the queue could not be
    /// opened, and then nothing can be undone.
    let queue: ChangeQueue?

    /// The add or edit sheet, while it is up.
    private(set) var form: EntryFormPurpose?
    /// The open entry whose delete is waiting for its inline confirm.
    private(set) var confirmingDelete: UUID?
    private(set) var undoOffer: UndoOffer?

    @ObservationIgnored private let live: LiveStore
    @ObservationIgnored private let copies: CopyCache?
    @ObservationIgnored private let requests: Signal<ScreenRequest>

    init(writer: StoreWriter, queue: ChangeQueue?, live: LiveStore, copies: CopyCache?,
         requests: Signal<ScreenRequest>) {
        self.writer = writer
        self.queue = queue
        self.live = live
        self.copies = copies
        self.requests = requests
    }

    // MARK: The form

    func startAdd(folder: String) {
        guard form == nil else { return }
        form = .add(folder: folder)
    }

    func startEdit(_ session: OpenSession, focusName: Bool = false) {
        guard form == nil else { return }
        confirmingDelete = nil
        form = .edit(sessionID: session.id, focusName: focusName)
    }

    func formClosed() {
        form = nil
    }

    /// A blank form for "New password", following the store.
    func addForm(folder: String) -> EntryFormModel {
        EntryFormModel(adding: folder, writer: writer, folders: allFolders, live: live)
    }

    /// The form for `session`, filled in. Throws if it was closed.
    func editForm(_ session: OpenSession, focusName: Bool) throws -> EntryFormModel {
        try EntryFormModel(editing: session, focusName: focusName, writer: writer, folders: allFolders, live: live)
    }

    /// Every folder in the store, for the folder picker, sorted.
    var allFolders: [String] {
        live.store.allFolders().sorted { $0.localizedStandardCompare($1) == .orderedAscending }
    }

    /// Save in the form. A new entry needs the key to open, like any
    /// other; an edited one stays on screen with the text just saved.
    /// `false` if the form refused or said why not.
    func save(_ form: EntryFormModel) -> Bool {
        guard let saved = form.save(copies: copies) else { return false }
        let renamed = saved.previous.map { StoreChange.Renamed(from: $0, to: saved.entry) }
        live.announce(.wrote(entries: [saved.previous, saved.entry].compactMap { $0 }, renamed: renamed))
        if case .edit(let id, _) = form.purpose {
            requests.send(.saved(sessionID: id, saved))
        }
        requests.send(.toast(saved.commits.isEmpty ? "Nothing changed." : "Saved. Not sent yet."))
        if let notice = followUpNotice(saved.followUpFailures) {
            requests.send(.notice(notice))
        }
        return true
    }

    /// "Accept the new list" for `folder`.
    func acceptRecipients(folder: String) throws {
        try writer.acceptRecipients(folder: folder)
        live.announce(.recipientsAccepted(folder: folder))
    }

    // MARK: Deleting and undoing

    func confirmDelete(_ session: OpenSession) {
        confirmingDelete = session.id
    }

    func keepEntry() {
        confirmingDelete = nil
    }

    /// Delete after its inline confirm: back to the list, with Undo.
    func delete(_ session: OpenSession) {
        confirmingDelete = nil
        let deleted: Deleted
        do {
            deleted = try BackgroundTime.run("Delete") { try writer.delete(entry: session.ref.path, copies: copies) }
        } catch let error as WriteError {
            let words = editProblem(error, folder: session.ref.folder)
            requests.send(.notice(Notice(problem: words.problem, nextStep: words.nextStep)))
            return
        } catch {
            requests.send(.notice(Notice(problem: "\(session.ref.name) wasn't deleted.", nextStep: describe(error))))
            return
        }
        live.announce(.wrote(entries: [session.ref.path], renamed: nil))
        requests.send(.deleted(sessionID: session.id))
        if let notice = followUpNotice(deleted.followUpFailures) {
            requests.send(.notice(notice))
        }
        if queue != nil {
            undoOffer = UndoOffer(
                message: "Deleted \(session.ref.name).", entryName: session.ref.name, change: deleted.commit)
        }
    }

    /// Undo on the delete toast: only if that change is still the newest.
    func undo(_ offer: UndoOffer) {
        guard let queue else { return }
        undoOffer = nil
        guard queue.unpushed.first?.id == offer.change else {
            requests.send(.toast("Not undone: other changes came after it."))
            return
        }
        if queue.undoLast(task: "Undo") {
            requests.send(.toast("\(offer.entryName) is back."))
        } else {
            requests.send(.notice(Notice(problem: "Not undone", nextStep: queue.problem ?? "")))
        }
    }

    /// The Undo toast's time is up.
    func undoOfferEnded(_ offer: UndoOffer) {
        if undoOffer?.id == offer.id {
            undoOffer = nil
        }
    }

    // MARK: The unpushed list

    func undoLastChange() {
        changeQueue { $0.undoLast(task: "Change") }
    }

    func discardChange(_ id: String) {
        changeQueue { $0.discard(id) }
    }

    func discardAllChanges() {
        changeQueue { $0.discardAll() }
    }

    private func changeQueue(_ change: (ChangeQueue) -> Bool) {
        guard let queue else { return }
        undoOffer = nil
        _ = change(queue)
    }
}
