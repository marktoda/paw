import SwiftUI

/// iPad in regular width: Home in the sidebar, a folder or the search
/// results in the middle, the open entry on the right. The columns are
/// read from `model.path` (`SplitPath`), the list the iPhone's stack uses,
/// so the same screens, identifiers and closing rules serve both.
struct SplitRootView: View {
    @Bindable var model: AppModel
    @State private var columns = NavigationSplitViewVisibility.all
    @State private var query = ""
    @FocusState private var searchFocused: Bool

    private var split: SplitPath {
        SplitPath(model.path)
    }

    private var trimmedQuery: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var searchPrompt: String {
        searchPromptWords(entryCount: model.storeSession.live.entryCount)
    }

    var body: some View {
        NavigationSplitView(columnVisibility: $columns) {
            HomeScreen(model: model, inSidebar: true)
                .navigationSplitViewColumnWidth(min: 280, ideal: 320, max: 400)
        } content: {
            NavigationStack(path: folderStack) {
                middle
                    .navigationDestination(for: Route.self) { route in
                        RouteScreen(model: model, route: route)
                    }
            }
            .navigationSplitViewColumnWidth(min: 300, ideal: 360, max: 460)
        } detail: {
            NavigationStack(path: detailStack) {
                detail
                    .navigationDestination(for: Route.self) { route in
                        RouteScreen(model: model, route: route)
                            .padReadableWidth()
                    }
            }
        }
        .focusedSceneValue(\.pawActions, actions)
    }

    // MARK: Columns

    /// Search results while something is typed, else the folder picked in
    /// the sidebar. The search field stays put while the two swap.
    private var middle: some View {
        Group {
            if !trimmedQuery.isEmpty {
                SearchResults(model: model, query: trimmedQuery)
            } else if case .folder(let path)? = split.folders.first {
                FolderScreen(model: model, path: path)
                    .id(path)
            } else {
                EmptyState(symbol: PawSymbol.folder, title: "Choose a folder or password", message: "")
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("split-no-folder")
            }
        }
        .searchable(text: $query, prompt: Text(searchPrompt))
        .searchFocused($searchFocused)
    }

    /// The entry (or other screen) on the right. A new entry is a new view,
    /// so nothing the last one revealed carries over.
    @ViewBuilder private var detail: some View {
        if let first = split.details.first {
            RouteScreen(model: model, route: first)
                .padReadableWidth()
                .id(first)
        } else {
            EmptyState(symbol: PawSymbol.entry, title: "Choose a password", message: "")
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("split-no-entry")
        }
    }

    /// Subfolders pushed in the middle column, below the sidebar's pick.
    private var folderStack: Binding<[Route]> {
        Binding(
            get: { trimmedQuery.isEmpty ? Array(SplitPath(model.path).folders.dropFirst()) : [] },
            set: { stack in
                guard trimmedQuery.isEmpty else { return }
                let split = SplitPath(model.path)
                model.setPath(Array(split.folders.prefix(1)) + stack + split.details)
            })
    }

    /// Screens pushed on the right, above the first.
    private var detailStack: Binding<[Route]> {
        Binding(
            get: { Array(SplitPath(model.path).details.dropFirst()) },
            set: { stack in
                let split = SplitPath(model.path)
                model.setPath(split.folders + Array(split.details.prefix(1)) + stack)
            })
    }

    // MARK: Keyboard

    /// What the shortcuts do (`PawCommands`): what the buttons do, and
    /// nothing while a sheet is up, even one that came up after these were
    /// published.
    private var actions: PawActions {
        let model = model
        let syncing = model.storeSession.syncing
        let edits = model.storeSession.edits
        if model.sheetIsUp {
            return PawActions()
        }
        func unlessSheetUp(_ action: @escaping () -> Void) -> () -> Void {
            { if !model.sheetIsUp { action() } }
        }
        return PawActions(
            search: { searchFocused = true },
            add: edits.map { edits in unlessSheetUp { edits.startAdd(folder: model.splitFolder) } },
            check: syncing == nil || syncing?.running != nil ? nil : unlessSheetUp {
                guard let syncing, syncing.running == nil else { return }
                syncing.check()
            },
            lock: unlessSheetUp { model.lockNow() },
            settings: unlessSheetUp { model.showsSettings = true })
    }
}

extension View {
    /// The row whose folder or entry the next column shows. Only ever true
    /// in the split view.
    @ViewBuilder
    func splitSelected(_ selected: Bool) -> some View {
        if selected {
            // Stronger than `fill`: the sidebar's rows are already grey.
            listRowBackground(PawColor.action.opacity(0.12))
                .accessibilityAddTraits(.isSelected)
        } else {
            self
        }
    }
}
