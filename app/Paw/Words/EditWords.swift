import Foundation

/// A write or queue failure in words: what happened, then what to do.
struct EditProblem: Equatable {
    let problem: String
    let nextStep: String
    /// "Review who can open this folder" applies.
    var reviewFolder: String? = nil
}

/// A queue or host-key change that failed, in one sentence. The core
/// decides what went wrong; this only words it.
extension SyncError: WordedError {
    var sentence: String {
        switch self {
        case .UncommittedChanges(let paths):
            "The store has changes paw didn't make: \(paths.joined(separator: ", "))."
        case .Failed(let message), .RefusedRemote(_, let message), .PushRefused(let message),
             .RepositoryMissing(let message):
            capitalised(message) + "."
        case .StoreBusy:
            capitalised(storeBusyText) + "."
        case .HostKeyBundled(let host):
            capitalised(hostKeyBundled(host)) + "."
        case .Cancelled:
            "Stopped."
        default:
            String(describing: self)
        }
    }
}

/// A refused write in one sentence: `editProblem`'s first line.
extension WriteError: WordedError {
    var sentence: String {
        let problem = editProblem(self, folder: "").problem
        return problem.hasSuffix(".") ? problem : problem + "."
    }
}

/// "Who can open work changed": a folder's list of keys is not the one
/// accepted on this iPhone.
func whoCanOpenChanged(_ folder: String) -> String {
    "Who can open \(folder.isEmpty ? "your store" : folder) changed"
}

/// How a refused save, delete or rename reads, with the step that fixes it.
func editProblem(_ error: WriteError, folder: String) -> EditProblem {
    let place = folder.isEmpty ? "the top of your store" : folder
    switch error {
    case .MissingRecipients(_, let missing):
        let count = missing.count == 1 ? "a key" : "\(missing.count) keys"
        return EditProblem(
            problem: "Can't save in \(place)",
            nextStep: "It's also encrypted for \(count) paw doesn't have. Pick another folder.")
    case .RecipientsChanged:
        return EditProblem(
            problem: whoCanOpenChanged(folder),
            nextStep: "Review the new list before saving here.", reviewFolder: folder)
    case .RecipientsNotConfirmed:
        return EditProblem(
            problem: "paw hasn't confirmed who can open this store",
            nextStep: "Finish setting up the store, then save again.")
    case .NoSuchEntry(let entry):
        return EditProblem(
            problem: "\(EntryRef(path: entry).name) isn't in the store any more",
            nextStep: "It may have been deleted on another device.")
    case .AlreadyExists(let entry):
        return EditProblem(
            problem: "There's already a password called '\(EntryRef(path: entry).name)'",
            nextStep: "Pick another name.")
    case .ChangedSinceOpened(let entry):
        return EditProblem(
            problem: "\(EntryRef(path: entry).name) changed since you opened it",
            nextStep: "Nothing was saved. Close this, open it again and redo your change.")
    case .Invalid(let problems):
        return EditProblem(
            problem: problems.first?.message ?? "Something here can't be saved",
            nextStep: "Fix it, then save.")
    case .Policy(let message):
        return EditProblem(problem: "That password can't be generated", nextStep: message)
    case .Failed(let message), .Certificate(let message):
        return EditProblem(problem: "Couldn't save this", nextStep: "Nothing was changed. Details: \(message)")
    case .StoreBusy:
        return EditProblem(problem: "Couldn't save this", nextStep: "Nothing was changed. Details: \(storeBusyText)")
    case .NoEncryptionKey(_, let reason):
        return EditProblem(problem: "A key for this folder can't encrypt", nextStep: reason)
    case .NotPinned:
        return EditProblem(problem: Device.words("A key for this folder isn't saved on this iPhone"), nextStep: "Nothing was changed.")
    }
}
