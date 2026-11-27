import Foundation

/// An unsent change as the screens name it.
extension QueueItem {
    /// "Added github.com", "Renamed vpn to vpn-old".
    var title: String {
        let name = EntryRef(path: entry).name
        switch kind {
        case .added: return "Added \(name)"
        case .edited: return "Edited \(name)"
        case .deleted: return "Deleted \(name)"
        case .renamed:
            guard let previous else { return "Renamed \(name)" }
            let before = EntryRef(path: previous)
            let after = EntryRef(path: entry)
            return before.folder == after.folder
                ? "Renamed \(before.name) to \(after.name)"
                : "Moved \(after.name) to \(after.folder.isEmpty ? "the top" : after.folder)"
        }
    }

    /// What discarding it does.
    var discardDetail: String {
        let name = EntryRef(path: entry).name
        switch kind {
        case .added: return "\(name) is removed from this \(Device.name)."
        case .edited: return "\(name) goes back to how it was."
        case .deleted: return "\(name) comes back."
        case .renamed: return "\(name) gets its old name back."
        }
    }
}

/// "14:02" today, "yesterday", "Mon" this week, else "3 Nov": when an
/// unsent change was saved.
func savedWhen(_ seconds: Int64, now: Date = Date()) -> String {
    let date = Date(timeIntervalSince1970: TimeInterval(seconds))
    let day = pastDay(date, now: now)
    if !day.today, Calendar.current.isDateInYesterday(date) {
        return "yesterday"
    }
    return day.words
}
