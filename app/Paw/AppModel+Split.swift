import Foundation

/// The iPad split view's columns, read from the one route list that the
/// iPhone's stack uses (`AppModel.path`): the folders shown in the middle
/// column, then the screens shown on the right. The same list works as a
/// stack, so moving between the split view and the stack (Slide Over, a
/// narrow window) loses nothing.
///
/// In the split view an entry replaces whatever was on the right, so the
/// entry it replaces leaves `path` and `pathChanged` closes it, as going
/// back does on iPhone.
struct SplitPath {
    /// `.folder` routes: the top-level folder picked in the sidebar, then
    /// the subfolders opened inside it.
    private(set) var folders: [Route] = []
    /// Everything else, in order: what the right-hand column shows.
    private(set) var details: [Route] = []

    init(_ path: [Route]) {
        for route in path {
            switch route {
            case .folder(let folder):
                // Only the sidebar shows top-level folders: picking one
                // starts the middle column again, and picking another
                // closes the open entry, whose row is no longer shown.
                if !folder.contains("/") {
                    if folders.first != route {
                        details.removeAll { $0.showsEntry }
                    }
                    folders = []
                }
                folders.append(route)
            case .entry, .anotherKey, .unpushed:
                details = [route]
            case .sync:
                details.removeAll { $0.showsEntry }
                details.append(route)
            }
        }
    }

    var path: [Route] {
        folders + details
    }

    /// The folder the middle column shows, `nil` when none is picked.
    var folder: String? {
        if case .folder(let folder)? = folders.last { return folder }
        return nil
    }
}

extension Route {
    /// An entry, open or explained: one at a time in the split view.
    var showsEntry: Bool {
        switch self {
        case .entry, .anotherKey: true
        default: false
        }
    }
}

extension AppModel {
    /// The root picked the split view (iPad, regular width) or the stack.
    func setSplitView(_ on: Bool) {
        usesSplitView = on
        if on {
            pathChanged()
        }
    }

    /// The open entry on the right of the split view, if any.
    var splitShownEntry: OpenSession? {
        guard usesSplitView, case .entry(let id)? = SplitPath(path).details.last else { return nil }
        return session(id)
    }

    /// The path of the entry on the right, open or closed, for its row's
    /// selected look.
    var splitShownPath: String? {
        guard usesSplitView, case .entry(let id)? = SplitPath(path).details.last else { return nil }
        return session(id)?.ref.path ?? closedEntry(id)?.ref.path
    }

    /// The folder the middle column shows: where ⌘N adds.
    var splitFolder: String {
        usesSplitView ? (SplitPath(path).folder ?? "") : ""
    }

    /// Split view: a new open replaces the entry on the right, so that one
    /// closes now, as going back to the list does on iPhone, before any
    /// key sheet or Face ID prompt for the next.
    func closeSplitEntryForNextOpen() {
        guard usesSplitView else { return }
        let split = SplitPath(path)
        guard split.details.contains(where: \.showsEntry) else { return }
        path = split.folders + split.details.filter { !$0.showsEntry }
        pathChanged()
    }
}
