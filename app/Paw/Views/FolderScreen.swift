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
                        kind: .error, problem: "This folder couldn't be read.",
                        nextStep: describe(error))
                        .pawListRow()
                }
                .listStyle(.insetGrouped)
            }
        }
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .navigationTitle(EntryRef(path: path).name)
        .navigationBarTitleDisplayMode(.large)
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
        }
        .listStyle(.insetGrouped)
    }
}
