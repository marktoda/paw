import SwiftUI

/// Home: the title, what opens with Face ID, what was opened recently and
/// now needs the key, the folders and the entries in no folder. Typing in
/// the bottom search field swaps the list for ranked results.
struct HomeScreen: View {
    let model: AppModel
    @State private var query = ""

    var body: some View {
        Group {
            if trimmedQuery.isEmpty {
                home
            } else {
                SearchResults(model: model, query: trimmedQuery)
            }
        }
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        // Adding passwords comes in a later milestone.
        .pawBottomSearch(text: $query, prompt: searchPrompt, addDisabled: true) {}
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    model.showsSettings = true
                } label: {
                    Label("Settings", systemImage: PawSymbol.settings)
                }
                .accessibilityIdentifier("settings-button")
            }
        }
        .onAppear { model.refreshCopies() }
    }

    private static func faceIdSubtitle(_ entry: EntryRef, untilMs: Int64) -> String {
        let until = "until \(clockTime(ms: untilMs))"
        return entry.folder.isEmpty ? until : "\(entry.folder) \u{00B7} \(until)"
    }

    private var trimmedQuery: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var searchPrompt: String {
        model.entryCount == 1 ? "Search 1 password" : "Search \(model.entryCount) passwords"
    }

    private var home: some View {
        List {
            Section {
                Text("Passwords")
                    .font(PawFont.homeTitle)
                    .foregroundStyle(PawColor.label)
                    .accessibilityAddTraits(.isHeader)
                    .accessibilityIdentifier("home-title")
                    .padding(.horizontal, PawMetrics.Spacing.xs)
                    .listRowInsets(EdgeInsets())
                    .listRowBackground(Color.clear)
            }
            NoticeSection(model: model)
            if let problem = model.copiesProblem {
                Section {
                    ErrorRow(
                        kind: .warning, problem: problem,
                        nextStep: "Every password can still be opened with your key.")
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
                    PawSectionHeader("Recent, needs your key")
                }
            }
            if !model.home.folders.isEmpty {
                Section {
                    ForEach(model.home.folders, id: \.path) { folder in
                        FolderRow(name: folder.name, count: Int(folder.entryCount)) {
                            model.path.append(.folder(folder.path))
                        }
                        .pawListRow()
                    }
                } header: {
                    PawSectionHeader("Folders")
                }
            }
            if !model.home.entries.isEmpty {
                Section {
                    ForEach(model.home.entries, id: \.path) { entry in
                        EntryRowFor(model: model, entry: entry, subtitle: "")
                    }
                } header: {
                    PawSectionHeader("Not in a folder")
                }
            }
            if model.entryCount == 0 {
                Section {
                    EmptyState(
                        symbol: PawSymbol.entry, title: "Your store is empty",
                        message: "Passwords you add on a computer appear here after a sync.")
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
private struct SearchResults: View {
    let model: AppModel
    let query: String

    var body: some View {
        let hits = model.search(query)
        if hits.isEmpty {
            EmptyState(
                symbol: PawSymbol.search,
                title: "No entry or folder called \u{201C}\(query)\u{201D}",
                message: "Search looks at names only, never inside entries.")
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
                    PawSectionHeader(hits.count == 1 ? "1 match" : "\(hits.count) matches")
                        .accessibilityIdentifier("search-count")
                } footer: {
                    Text("Search looks at names and folders, never inside entries.")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
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
