import SwiftUI

/// The row in Settings, just under AutoFill.
struct SitesUsedSettingsSection: View {
    let model: AppModel

    var body: some View {
        if let stateDir = model.storeSession.edits?.stateDir {
            Section {
                NavigationLink {
                    SitesUsedScreen(model: model, pairings: SitePairings.open(stateDir: stateDir))
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
    let model: AppModel
    let pairings: SitePairings
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
        #if DEBUG && targetEnvironment(simulator)
        SimulatedSitePairings.seedOnce(pairings, store: model.store)
        #endif
        rows = pairings.pairings().sorted { $0.entry < $1.entry }
    }

    private func forget(entry: String, site: String) {
        changed { try pairings.forgetSite(entry: entry, site: site) }
    }

    private func clearAll() {
        confirmingClear = false
        changed { try pairings.clearPairings(entry: nil) }
    }

    /// Makes the change, shows the list again and writes the AutoFill
    /// suggestions again, as paw does at launch.
    private func changed(_ change: () throws -> Void) {
        do {
            try change()
            problem = nil
        } catch {
            problem = describe(error)
        }
        rows = pairings.pairings().sorted { $0.entry < $1.entry }
        model.storeSession.autoFill?.republish()
    }
}
