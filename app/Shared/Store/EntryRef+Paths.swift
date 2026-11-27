import Foundation

extension EntryRef {
    /// An entry from its full path, e.g. `web/github.com`.
    init(path: String) {
        self = entryRef(path: path)
    }

    /// The top-level folder, which picks the tint; `nil` at the top.
    var topFolder: String? {
        topFolderOf(folder: folder)
    }
}

extension HighlightRange {
    var nsRange: NSRange {
        NSRange(location: Int(start), length: Int(len))
    }
}
