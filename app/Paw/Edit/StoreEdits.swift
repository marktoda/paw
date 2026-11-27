import Foundation
import Observation

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

    private(set) var form: EntryFormPurpose?
    /// The open entry whose delete is waiting for its inline confirm.
    private(set) var confirmingDelete: UUID?
    /// The last delete's Undo, until it is taken or its toast goes.
    private var offered: UndoOffer?

    @ObservationIgnored private let live: LiveStore
    @ObservationIgnored private let copies: LiveCopies
    @ObservationIgnored private let requests: Signal<ScreenRequest>

    init(writer: StoreWriter, queue: ChangeQueue?, live: LiveStore, copies: LiveCopies,
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

    func addForm(folder: String) -> EntryFormModel {
        EntryFormModel(adding: folder, writer: writer, folders: allFolders, live: live)
    }

    func editForm(_ session: OpenSession, focusName: Bool) throws -> EntryFormModel {
        try EntryFormModel(editing: session, focusName: focusName, writer: writer, folders: allFolders, live: live)
    }

    var allFolders: [String] {
        live.store.allFolders().sorted { $0.localizedStandardCompare($1) == .orderedAscending }
    }

    /// Save in the form. A new entry needs the key to open, like any
    /// other; an edited one stays on screen with the text just saved.
    /// `false` if the form refused or said why not.
    func save(_ form: EntryFormModel) -> Bool {
        guard let saved = write("Save", { form.save(copies: copies.cache) }) else { return false }
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

    func acceptRecipients(folder: String) throws {
        try write("Accept list") { try writer.acceptRecipients(folder: folder) }
        live.announce(.recipientsAccepted(folder: folder))
    }

    /// Every write here, and the queue's (`ChangeQueue.change`): it takes
    /// the store lock, so it runs inside background time (`BackgroundTime`).
    private func write<T>(_ name: String, _ work: () throws -> T) rethrows -> T {
        try BackgroundTime.run(name, work)
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
            deleted = try write("Delete") { try writer.delete(entry: session.ref.path, copies: copies.cache) }
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
        offered = UndoOffer(
            message: "Deleted \(session.ref.name).", entryName: session.ref.name, change: deleted.commit)
    }

    /// Undo is offered while its change is the newest one waiting: a change
    /// undone, discarded, sent or made after it ends the offer.
    var undoOffer: UndoOffer? {
        guard let offered, queue?.unpushed.first?.id == offered.change else { return nil }
        return offered
    }

    /// Undo on the delete toast.
    func undo(_ offer: UndoOffer) {
        guard let queue, undoOffer?.id == offer.id else { return }
        offered = nil
        if queue.undoLast() {
            requests.send(.toast("\(offer.entryName) is back."))
        } else {
            requests.send(.notice(Notice(problem: "Not undone", nextStep: queue.problem ?? "")))
        }
    }

    func undoOfferEnded(_ offer: UndoOffer) {
        if offered?.id == offer.id {
            offered = nil
        }
    }
}
