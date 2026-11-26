import Foundation

/// Adding, editing, deleting and the unpushed queue. Every call here works
/// without the key and without the network: encryption uses the public
/// keys pinned at setup, and each change is one local commit.
extension AppModel {
    func startAdd(folder: String) {
        guard let edits, edits.form == nil else { return }
        edits.form = .add(folder: folder)
    }

    func startEdit(_ session: OpenSession, focusName: Bool = false) {
        guard let edits, edits.form == nil else { return }
        edits.confirmingDelete = nil
        edits.form = .edit(sessionID: session.id, focusName: focusName)
    }

    /// Save in the form. A new entry needs the key to open, like any
    /// other; an edited one stays on screen with the text just saved.
    /// `false` if the form refused or said why not.
    func save(_ form: EntryFormModel) -> Bool {
        guard let edits, let outcome = edits.save(form, copies: copies) else { return false }
        if case .edit(let id, _) = form.purpose {
            if let opened = outcome.opened {
                replaceSession(id, ref: EntryRef(path: outcome.entry), entry: opened)
            } else {
                // Saved, but the screen can't show the new text: close it.
                leave(.entry(id))
            }
        }
        toast = outcome.commits.isEmpty ? "Nothing changed." : "Saved. Not sent yet."
        return true
    }

    /// Delete after its inline confirm: back to the list, with Undo.
    func delete(_ session: OpenSession) {
        guard let edits else { return }
        edits.confirmingDelete = nil
        do {
            try edits.delete(session.ref.path, copies: copies)
        } catch let error as WriteError {
            notice = Notice(
                problem: editProblem(error, folder: session.ref.folder).problem,
                nextStep: editProblem(error, folder: session.ref.folder).nextStep)
            return
        } catch {
            notice = Notice(problem: "\(session.ref.name) wasn't deleted.", nextStep: describe(error))
            return
        }
        var routes = path
        if routes.last == .entry(session.id) {
            routes.removeLast()
        }
        // A folder whose last entry went is gone too.
        while case .folder(let folder)? = routes.last, (try? store.folder(path: folder)) == nil {
            routes.removeLast()
        }
        setPath(routes)
        if let newest = edits.unpushed.first {
            edits.undoOffer = UndoOffer(
                message: "Deleted \(session.ref.name).", entryName: session.ref.name, change: newest.id)
        }
    }

    /// Undo on the delete toast: only if that change is still the newest.
    func undo(_ offer: UndoOffer) {
        guard let edits, edits.sync != nil else { return }
        edits.undoOffer = nil
        edits.refreshQueue()
        guard edits.unpushed.first?.id == offer.change else {
            toast = "Not undone: other changes came after it."
            return
        }
        do {
            try edits.undoLast()
            toast = "\(offer.entryName) is back."
        } catch {
            edits.queueProblem = describe(error)
        }
    }

    // MARK: The unpushed list

    func undoLastChange() {
        changeQueue { try $0.undoLast(task: "Change") }
    }

    func discardChange(_ id: String) {
        changeQueue { try $0.discard(id) }
    }

    func discardAllChanges() {
        changeQueue { try $0.discardAll() }
    }

    private func changeQueue(_ change: (StoreEdits) throws -> Void) {
        guard let edits, edits.sync != nil else { return }
        edits.undoOffer = nil
        do {
            try change(edits)
            edits.queueProblem = nil
        } catch {
            edits.queueProblem = describe(error)
        }
    }

    /// Every folder in the store, for the folder picker, sorted.
    var allFolders: [String] {
        store.allFolders().sorted { $0.localizedStandardCompare($1) == .orderedAscending }
    }
}
