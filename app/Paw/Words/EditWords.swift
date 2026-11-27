import Foundation

/// A write or queue failure in words: what happened, then what to do.
struct EditProblem: Equatable {
    let problem: String
    let nextStep: String
    /// "Review who can open this folder" applies.
    var offersReview = false

    /// A password the options asked for couldn't be made, for `why`.
    static func notGenerated(_ why: String) -> EditProblem {
        EditProblem(problem: "That password can't be generated", nextStep: why)
    }
}

/// A refused write in one sentence: `editProblem`'s first line.
extension WriteError: WordedError {
    var sentence: String {
        let problem = editProblem(self, folder: "").problem
        return problem.hasSuffix(".") ? problem : problem + "."
    }
}

/// A folder in a sentence about who can open it: "work", or "your store"
/// for the top, whose list of keys is the whole store's.
func recipientsPlace(_ folder: String) -> String {
    folder.isEmpty ? "your store" : folder
}

/// A folder in a sentence about saving in it: "work", or "the top of
/// your store".
func savingPlace(_ folder: String) -> String {
    folder.isEmpty ? "the top of your store" : folder
}

/// "Who can open work changed": a folder's list of keys is not the one
/// accepted on this iPhone.
func whoCanOpenChanged(_ folder: String) -> String {
    "Who can open \(recipientsPlace(folder)) changed"
}

/// The step after `whoCanOpenChanged`: "here" on a form saving in the
/// folder, "there" elsewhere.
func reviewBeforeSaving(here: Bool) -> String {
    "Review the new list before saving \(here ? "here" : "there")."
}

/// A key on a folder's list that paw can't encrypt for.
enum MissingKeyWords {
    static let thisKey = "paw doesn't have this key"
    /// A folder paw can't save in, as a choice on the form.
    static let folder = "Needs a key paw doesn't have"
}

extension RecipientLine {
    /// "your key", or "key 9A1C 03F2" by the last 8 hex digits.
    var spokenName: String {
        switch status {
        case .ready: return "your key"
        default: return "key \(RecipientLineWords.ending(line))"
        }
    }

    /// How this line is satisfied, in words.
    var statusWords: String {
        switch status {
        case .ready: return "Your key"
        case .missing: return MissingKeyWords.thisKey
        }
    }
}

extension FolderProblem {
    /// Why paw can't save in `folder`, with the step that fixes it.
    func words(folder: String) -> EditProblem {
        switch self {
        case .missingKeys(let missing):
            let count = missing == 1 ? "a key" : "\(missing) keys"
            return EditProblem(
                problem: "Can't save in \(savingPlace(folder))",
                nextStep: "It's also encrypted for \(count) paw doesn't have. Pick another folder.")
        case .changed:
            return EditProblem(
                problem: whoCanOpenChanged(folder),
                nextStep: reviewBeforeSaving(here: true), offersReview: true)
        case .notConfirmed:
            return EditProblem(
                problem: "paw hasn't confirmed who can open this store",
                nextStep: "Finish setting up the store, then save.")
        }
    }
}

/// How a refused save, delete or rename reads, with the step that fixes it.
func editProblem(_ error: WriteError, folder: String) -> EditProblem {
    switch error {
    case .MissingRecipients(_, let missing):
        return FolderProblem.missingKeys(missing.count).words(folder: folder)
    case .RecipientsChanged:
        return FolderProblem.changed.words(folder: folder)
    case .RecipientsNotConfirmed:
        return FolderProblem.notConfirmed.words(folder: folder)
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
        return .notGenerated(message)
    case .Fault(.storeBusy):
        return EditProblem(problem: StoreBusyWords.problem, nextStep: StoreBusyWords.nextStep)
    case .Fault(let fault):
        return EditProblem(problem: "Couldn't save this", nextStep: "Nothing was changed. Details: \(faultDetail(fault))")
    }
}

/// What a save or a delete left undone beside the entry, said on Home;
/// `nil` when it all was done. The first one is said: they share a cause.
func followUpNotice(_ failures: [FollowUpFailure]) -> Notice? {
    guard let failure = failures.first else { return nil }
    let problem = switch failure.followUp {
    case .faceIdCopy: "Face ID's copy wasn't updated"
    case .sitePairings, .oneTimeCodeMark: "AutoFill wasn't updated"
    }
    return Notice(problem: problem, nextStep: asSentence(failure.message))
}
