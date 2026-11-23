import SwiftUI

/// One folder: its subfolders first, then its entries, in the same rows as Home.
struct FolderScreen: View {
    let model: AppModel
    let path: String

    var body: some View {
        Group {
            switch model.listing(path) {
            case .success(let listing):
                list(listing)
            case .failure(let error):
                List {
                    ErrorRow(
                        kind: .error, problem: "Couldn't read this folder",
                        nextStep: describe(error))
                        .pawListRow()
                }
                .listStyle(.insetGrouped)
            }
        }
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .navigationTitle(EntryRef(path: path).name)
        .navigationBarTitleDisplayMode(.large)
        .addEntryButton(model, folder: path)
        .onAppear { model.refreshCopies() }
    }

    private func list(_ listing: FolderListing) -> some View {
        // Subfolders take the tint of the top-level folder they are in.
        let tint = FolderTint(folder: path.split(separator: "/").first.map(String.init))
        return List {
            NoticeSection(model: model)
            if !listing.folders.isEmpty {
                Section {
                    ForEach(listing.folders, id: \.path) { folder in
                        FolderRow(name: folder.name, count: Int(folder.entryCount), tint: tint) {
                            model.path.append(.folder(folder.path))
                        }
                        .pawListRow()
                    }
                } header: {
                    PawSectionHeader("Folders")
                }
            }
            if !listing.entries.isEmpty {
                Section {
                    ForEach(listing.entries, id: \.path) { entry in
                        EntryRowFor(model: model, entry: entry, subtitle: "")
                    }
                } header: {
                    PawSectionHeader(listing.entries.count == 1 ? "1 password" : "\(listing.entries.count) passwords")
                }
            }
            if listing.folders.isEmpty, listing.entries.isEmpty {
                Section {
                    EmptyState(symbol: PawSymbol.folder, title: "Nothing here", message: "Tap + to add a password.")
                        .listRowBackground(Color.clear)
                }
            }
        }
        .listStyle(.insetGrouped)
    }
}
