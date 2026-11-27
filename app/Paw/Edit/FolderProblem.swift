import Foundation

/// Why paw can't save in a folder now: read off the folder's
/// `FolderRecipients` before a save, or off the `WriteError` that refused it.
enum FolderProblem: Equatable {
    /// Setup hasn't recorded who can open the store.
    case notConfirmed
    /// The folder's list of keys is not the one accepted on this iPhone.
    case changed
    /// It is also encrypted for this many keys paw doesn't have.
    case missingKeys(Int)
}

extension FolderRecipients {
    /// The lines paw has no key for.
    var missingKeys: Int {
        lines.filter { !$0.isReady }.count
    }

    /// `nil` when the core says paw can save here.
    var problem: FolderProblem? {
        if writable { return nil }
        if !confirmed { return .notConfirmed }
        if changed { return .changed }
        return .missingKeys(missingKeys)
    }
}

extension RecipientLine {
    var isReady: Bool {
        if case .ready = status { return true }
        return false
    }
}
