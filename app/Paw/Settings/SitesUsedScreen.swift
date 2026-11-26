import SwiftUI

struct SitesUsedSettingsSection: View {
    let model: AppModel

    var body: some View {
        if let autoFill = model.storeSession.autoFill {
            Section {
                NavigationLink {
                    SitesUsedScreen(autoFill: autoFill)
                } label: {
                    Text("Sites used")
                        .foregroundStyle(PawColor.label)
                }
                .accessibilityIdentifier("settings-sites-used")
            } footer: {
                Text("Sites you confirmed by hand.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
    }
}

/// Settings › Sites used: each password named without a domain
/// (`bank/northwind`) and the sites AutoFill filled it on, which it then
/// fills on without asking (core `SitePairings`). Removing one means the
/// next fill there asks again. Names and domains only.
struct SitesUsedScreen: View {
    let autoFill: AutoFillSuggestions
    @State private var rows: [Pairing] = []
    @State private var confirmingClear = false
    @State private var problem: String?

    var body: some View {
        List {
            if let problem {
                ErrorRow(kind: .error, problem: "Couldn't change sites used", nextStep: problem)
            }
            if rows.isEmpty {
                Text("None yet.")
                    .font(PawFont.subheadline)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("sites-used-empty")
            } else {
                ForEach(rows, id: \.entry) { pairing in
                    Section {
                        ForEach(pairing.sites, id: \.self) { site in
                            Text(site)
                                .font(PawFont.body)
                                .foregroundStyle(PawColor.label)
                                .accessibilityIdentifier("sites-used-site")
                                .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                                    Button("Remove", role: .destructive) {
                                        forget(entry: pairing.entry, site: site)
                                    }
                                    .accessibilityIdentifier("sites-used-remove")
                                }
                        }
                    } header: {
                        PawSectionHeader(pairing.entry)
                    }
                }
                Section {
                    if confirmingClear {
                        InlineConfirmRow(
                            question: "Clear all sites?", detail: "The next fill on each asks again.",
                            confirmTitle: "Clear all", cancelTitle: "Keep them",
                            identifier: "sites-used-clear",
                            onConfirm: clearAll,
                            onCancel: { confirmingClear = false })
                    } else {
                        Button("Clear all\u{2026}") { confirmingClear = true }
                            .foregroundStyle(PawColor.danger)
                            .accessibilityIdentifier("sites-used-clear-all")
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Sites used")
        .navigationBarTitleDisplayMode(.inline)
        .onAppear(perform: load)
    }

    private func load() {
        rows = autoFill.sites
    }

    private func forget(entry: String, site: String) {
        changed { try autoFill.forgetSite(entry: entry, site: site) }
    }

    private func clearAll() {
        confirmingClear = false
        changed { try autoFill.forgetAllSites() }
    }

    private func changed(_ change: () throws -> Void) {
        do {
            try change()
            problem = nil
        } catch {
            problem = describe(error)
        }
        rows = autoFill.sites
    }
}
