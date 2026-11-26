import SwiftUI

/// Home: the title, what opens with Face ID, what was opened recently and
/// now needs the key, the folders and the entries in no folder. Typing in
/// the bottom search field swaps the list for ranked results.
struct HomeScreen: View {
    let model: AppModel
    /// The iPad split view's sidebar: search and results are in the middle
    /// column, Add is at the top (the only one: it adds to the folder the
    /// middle column shows, as ⌘N does).
    var inSidebar = false
    @State private var query = ""

    var body: some View {
        if inSidebar {
            sidebar
        } else {
            stackRoot
        }
    }

    private var stackRoot: some View {
        Group {
            if trimmedQuery.isEmpty {
                home
            } else {
                SearchResults(model: model, query: trimmedQuery)
            }
        }
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .pawBottomSearch(text: $query, prompt: searchPrompt, addDisabled: model.storeSession.edits == nil) {
            model.storeSession.edits?.startAdd(folder: "")
        }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                settingsButton
            }
        }
        .onAppear { model.refreshCopies() }
    }

    private var sidebar: some View {
        home
            .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        model.storeSession.edits?.startAdd(folder: model.splitFolder)
                    } label: {
                        Label("Add a password", systemImage: PawSymbol.add)
                    }
                    .disabled(model.storeSession.edits == nil)
                    .accessibilityIdentifier("add-button")
                }
                ToolbarItem(placement: .topBarTrailing) {
                    settingsButton
                }
            }
            .onAppear { model.refreshCopies() }
    }

    private var settingsButton: some View {
        Button {
            model.showsSettings = true
        } label: {
            Label("Settings", systemImage: PawSymbol.settings)
        }
        .accessibilityIdentifier("settings-button")
    }

    private static func faceIdSubtitle(_ entry: EntryRef, untilMs: Int64) -> String {
        let until = "until \(clockTime(ms: untilMs))"
        return entry.folder.isEmpty ? until : "\(entry.folder) \u{00B7} \(until)"
    }

    private var trimmedQuery: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var searchPrompt: String {
        searchPromptWords(entryCount: model.storeSession.live.entryCount)
    }

    private var home: some View {
        List {
            Section {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
                    Text("Passwords")
                        .font(PawFont.homeTitle)
                        .foregroundStyle(PawColor.label)
                        .accessibilityAddTraits(.isHeader)
                        .accessibilityIdentifier("home-title")
                    HomeSyncPill(model: model)
                }
                .padding(.horizontal, PawMetrics.Spacing.xs)
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            }
            UnpushedSection(model: model)
            SyncSummarySection(model: model)
            NoticeSection(model: model)
            if let problem = model.copiesProblem {
                Section {
                    ErrorRow(
                        kind: .warning, problem: problem,
                        nextStep: "Passwords still open with your key.")
                        .pawListRow()
                }
            }
            if !model.liveCopies.isEmpty {
                Section {
                    ForEach(model.liveCopies, id: \.entry) { copy in
                        let entry = EntryRef(path: copy.entry)
                        EntryRowFor(
                            model: model, entry: entry,
                            subtitle: Self.faceIdSubtitle(entry, untilMs: copy.untilMs),
                            identifier: "face-id-\(entry.name)")
                    }
                } header: {
                    PawSectionHeader("Open with Face ID")
                }
            }
            let recent = model.recentNeedingKey
            if !recent.isEmpty {
                Section {
                    ForEach(recent, id: \.path) { entry in
                        EntryRowFor(
                            model: model, entry: entry, subtitle: entry.folder,
                            identifier: "recent-\(entry.name)")
                    }
                } header: {
                    PawSectionHeader("Recent")
                }
            }
            if !model.storeSession.live.home.folders.isEmpty {
                Section {
                    ForEach(model.storeSession.live.home.folders, id: \.path) { folder in
                        FolderRow(name: folder.name, count: Int(folder.entryCount)) {
                            model.show(.folder(folder.path))
                        }
                        .pawListRow()
                        .splitSelected(inSidebar && SplitPath(model.path).folders.first == .folder(folder.path))
                    }
                } header: {
                    PawSectionHeader("Folders")
                }
            }
            if !model.storeSession.live.home.entries.isEmpty {
                Section {
                    ForEach(model.storeSession.live.home.entries, id: \.path) { entry in
                        EntryRowFor(model: model, entry: entry, subtitle: "")
                    }
                } header: {
                    PawSectionHeader("Not in a folder")
                }
            }
            if model.storeSession.live.entryCount == 0 {
                Section {
                    EmptyState(
                        symbol: PawSymbol.entry, title: "No passwords yet",
                        message: "Tap + to add one.")
                        .listRowBackground(Color.clear)
                }
            }
        }
        .listStyle(.insetGrouped)
        .contentMargins(.top, 0, for: .scrollContent)
    }
}

/// An entry row with its glyph, tint and mark worked out from the model.
struct EntryRowFor: View {
    let model: AppModel
    let entry: EntryRef
    let subtitle: String
    var nameHighlights: [NSRange] = []
    var subtitleHighlights: [NSRange] = []
    var identifier: String? = nil

    var body: some View {
        let access = model.rowAccess(entry.path)
        EntryRow(
            name: entry.name,
            subtitle: access == .anotherKey ? "Encrypted for another key" : subtitle,
            symbol: EntryGlyph.symbol(for: entry.name),
            tint: FolderTint(folder: entry.topFolder),
            access: access,
            nameHighlights: nameHighlights,
            subtitleHighlights: access == .anotherKey ? [] : subtitleHighlights,
            identifier: identifier
        ) {
            model.open(entry)
        }
        .pawListRow()
        .splitSelected(model.splitShownPath == entry.path)
    }
}

/// A Face ID or key problem from the last tap, said where it happened.
struct NoticeSection: View {
    let model: AppModel

    var body: some View {
        if let notice = model.notice {
            Section {
                ErrorRow(
                    kind: .warning, problem: notice.problem, nextStep: notice.nextStep,
                    actionTitle: notice.keyEntry == nil ? nil : "Use your key",
                    actionNeedsKey: true
                ) {
                    if let entry = notice.keyEntry {
                        model.openWithKey(entry)
                    }
                }
                .pawListRow()
            }
        }
    }
}

/// Ranked results from the core, with what matched drawn semibold.
struct SearchResults: View {
    let model: AppModel
    let query: String

    var body: some View {
        let hits = model.storeSession.live.search(query)
        if hits.isEmpty {
            EmptyState(
                symbol: PawSymbol.search,
                title: "No password or folder called \u{201C}\(query)\u{201D}",
                message: "Search reads names, not what's inside.")
        } else {
            List {
                NoticeSection(model: model)
                Section {
                    ForEach(hits, id: \.entry.path) { hit in
                        EntryRowFor(
                            model: model, entry: hit.entry, subtitle: hit.entry.folder,
                            nameHighlights: hit.nameRanges.map(\.nsRange),
                            subtitleHighlights: hit.folderRanges.map(\.nsRange))
                    }
                } header: {
                    PawSectionHeader(counted(hits.count, "match", "matches"))
                        .accessibilityIdentifier("search-count")
                }
            }
            .listStyle(.insetGrouped)
        }
    }
}

extension HighlightRange {
    var nsRange: NSRange {
        NSRange(location: Int(start), length: Int(len))
    }
}
