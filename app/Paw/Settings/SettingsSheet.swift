import SwiftUI

/// Settings: Lock now, each feature's section, About and Remove, and in
/// the simulator the controls for the simulated hardware.
struct SettingsSheet: View {
    let model: AppModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Button {
                        model.lockNow()
                        dismiss()
                    } label: {
                        Text("Lock now")
                            .font(PawFont.body.weight(.semibold))
                            .foregroundStyle(PawColor.danger)
                    }
                    .accessibilityIdentifier("settings-lock-now")
                } footer: {
                    Text("Every password needs your key and its PIN again.")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
                AutoFillSettingsSection(suggestions: model.storeSession.autoFill)
                OneTimeCodeSettingsSection(model: model)
                SitesUsedSettingsSection(model: model)
                FaceIdSettingsSection(model: model)
                StoreSettingsSections(model: model)
                Section {
                    LabeledContent {
                        if let build = BuildStamp.words {
                            Text(build)
                                .font(PawFont.footnote.monospaced())
                                .foregroundStyle(PawColor.labelSecondary)
                                .textSelection(.enabled)
                        }
                    } label: {
                        Text("Build")
                            .foregroundStyle(PawColor.label)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("settings-build")
                    LicencesRow()
                } header: {
                    PawSectionHeader("About")
                }
                RemoveStoreSection(model: model)
                #if DEBUG && targetEnvironment(simulator)
                if model.simulator != nil {
                    SimulatorSettingsSection(model: model)
                }
                #endif
            }
            .listStyle(.insetGrouped)
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                        .padShortcut(.cancelAction)
                        .accessibilityIdentifier("settings-done")
                }
            }
        }
        // Tall from the start: Remove is the last row and must not open
        // below the fold.
        .presentationDetents([.large])
        .padFormSheet()
        .privacyCover()
    }
}
