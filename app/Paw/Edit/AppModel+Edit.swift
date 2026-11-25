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

    /// The store changed on disk: read it again, and the queue with it.
    func reloadAfterWrite(renamed: (from: String, to: String)? = nil) {
        guard let edits else { return }
        do {
            storeChanged(try PassStore(dir: edits.repoDir), renamed: renamed)
        } catch {
            notice = Notice(problem: "Couldn't read the list again", nextStep: describe(error))
        }
        edits.refreshQueue()
    }

    /// A save went through. A new entry needs the key to open, like any
    /// other; an edited one stays on screen with the text just saved.
    func saved(_ outcome: Saved, purpose: EntryFormPurpose) {
        let renamed = outcome.previous.map { (from: $0, to: outcome.entry) }
        reloadAfterWrite(renamed: renamed)
        if case .edit(let id, _) = purpose {
            if let opened = outcome.opened {
                replaceSession(id, ref: EntryRef(path: outcome.entry), entry: opened)
            } else {
                // Saved, but the screen can't show the new text: close it.
                path.removeAll { $0 == .entry(id) }
            }
        }
        toast = outcome.commits.isEmpty ? "Nothing changed." : "Saved. Not sent yet."
    }

    /// Delete after its inline confirm: back to the list, with Undo.
    func delete(_ session: OpenSession) {
        guard let edits else { return }
        edits.confirmingDelete = nil
        do {
            _ = try BackgroundTime.run("Delete") { try edits.writer.delete(entry: session.ref.path, copies: copies) }
        } catch let error as WriteError {
            notice = Notice(
                problem: editProblem(error, folder: session.ref.folder).problem,
                nextStep: editProblem(error, folder: session.ref.folder).nextStep)
            return
        } catch {
            notice = Notice(problem: "\(session.ref.name) wasn't deleted.", nextStep: describe(error))
            return
        }
        if let last = path.last, last == .entry(session.id) {
            path.removeLast()
        }
        reloadAfterWrite()
        // A folder whose last entry went is gone too.
        while case .folder(let folder)? = path.last, (try? store.folder(path: folder)) == nil {
            path.removeLast()
        }
        if let newest = edits.unpushed.first {
            edits.undoOffer = UndoOffer(
                message: "Deleted \(session.ref.name).", entryName: session.ref.name, change: newest.id)
        }
    }

    /// Undo on the delete toast: only if that change is still the newest.
    func undo(_ offer: UndoOffer) {
        guard let edits, let sync = edits.sync else { return }
        edits.undoOffer = nil
        edits.refreshQueue()
        guard edits.unpushed.first?.id == offer.change else {
            toast = "Not undone: other changes came after it."
            return
        }
        do {
            _ = try BackgroundTime.run("Undo") { try sync.undoLast() }
            reloadAfterWrite()
            toast = "\(offer.entryName) is back."
        } catch {
            edits.queueProblem = describeEditError(error)
        }
    }

    // MARK: The unpushed list

    func undoLastChange() {
        changeQueue { _ = try $0.undoLast() }
    }

    func discardChange(_ id: String) {
        changeQueue { _ = try $0.discard(id: id) }
    }

    func discardAllChanges() {
        changeQueue { try $0.discardAll() }
    }

    private func changeQueue(_ change: (StoreSync) throws -> Void) {
        guard let edits, let sync = edits.sync else { return }
        edits.undoOffer = nil
        do {
            try BackgroundTime.run("Change") { try change(sync) }
            reloadAfterWrite()
            edits.queueProblem = nil
        } catch {
            edits.queueProblem = describeEditError(error)
        }
    }

    /// Every folder in the store, for the folder picker, sorted.
    var allFolders: [String] {
        var found: [String] = []
        var pending = [home]
        while let listing = pending.popLast() {
            for folder in listing.folders {
                found.append(folder.path)
                if let inner = try? store.folder(path: folder.path) {
                    pending.append(inner)
                }
            }
        }
        return found.sorted { $0.localizedStandardCompare($1) == .orderedAscending }
    }
}
