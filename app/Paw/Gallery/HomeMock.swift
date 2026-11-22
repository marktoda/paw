#if DEBUG
import SwiftUI

/// Home as in the Keyring 2 mock-up, assembled from the components with
/// static sample data. Search filters the samples so the match style shows.
struct HomeMock: View {
    @State private var query = ""
    @State private var lastTapped = ""

    private var faceID: [GallerySample.Entry] {
        GallerySample.entries.filter { $0.access == .faceID }
    }

    private var recent: [GallerySample.Entry] {
        Array(GallerySample.entries.filter { $0.access == .needsKey }.prefix(2))
    }

    private var matches: [GallerySample.Entry] {
        GallerySample.entries.filter {
            $0.name.localizedCaseInsensitiveContains(query)
                || $0.folder.localizedCaseInsensitiveContains(query)
        }
    }

    var body: some View {
        Group {
            if query.isEmpty {
                home
            } else if matches.isEmpty {
                EmptyState(
                    symbol: PawSymbol.search,
                    title: "No matches",
                    message: "Search looks at names and folders. What is inside a password stays encrypted until you open it.")
            } else {
                results
            }
        }
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .pawBottomSearch(text: $query, prompt: "Search 332 passwords") {
            lastTapped = "Tapped Add"
        }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    lastTapped = "Tapped Settings"
                } label: {
                    Label("Settings", systemImage: PawSymbol.settings)
                }
                .accessibilityIdentifier("settings-button")
            }
        }
    }

    private var home: some View {
        List {
            Section {
                VStack(alignment: .leading, spacing: 14) {
                    VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
                        Text("Passwords")
                            .font(PawFont.homeTitle)
                            .foregroundStyle(PawColor.label)
                            .accessibilityAddTraits(.isHeader)
                            .accessibilityIdentifier("home-title")
                        SyncPill(status: .synced, text: "Synced 2 minutes ago")
                    }
                    .padding(.horizontal, PawMetrics.Spacing.xs)
                    UnpushedBanner(
                        reason: .waitingForKey,
                        title: "1 change is only on this iPhone",
                        detail: "Plug in your key to send it."
                    ) {
                        lastTapped = "Tapped the banner"
                    }
                }
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            }
            Section {
                ForEach(faceID) { entry in
                    GallerySample.row(entry) { lastTapped = "Tapped \(entry.name)" }
                        .pawListRow()
                }
            } header: {
                PawSectionHeader("Open with Face ID")
            }
            Section {
                ForEach(recent) { entry in
                    GallerySample.row(entry) { lastTapped = "Tapped \(entry.name)" }
                        .pawListRow()
                }
            } header: {
                PawSectionHeader("Recent, needs your key")
            }
            Section {
                ForEach(GallerySample.folders, id: \.name) { folder in
                    FolderRow(name: folder.name, count: folder.count) {
                        lastTapped = "Tapped folder \(folder.name)"
                    }
                    .pawListRow()
                }
            } header: {
                PawSectionHeader("Folders")
            } footer: {
                Text(lastTapped)
                    .accessibilityIdentifier("home-last-tapped")
            }
        }
        .listStyle(.insetGrouped)
        .contentMargins(.top, 0, for: .scrollContent)
    }

    private var results: some View {
        List {
            Section {
                ForEach(matches) { entry in
                    GallerySample.row(entry, match: query)
                        .pawListRow()
                }
            } header: {
                PawSectionHeader(matches.count == 1 ? "1 match" : "\(matches.count) matches")
                    .accessibilityIdentifier("search-count")
            } footer: {
                Text("Search looks at names and folders. What is inside a password stays encrypted until you open it.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
        .listStyle(.insetGrouped)
    }
}
#endif
