import SwiftUI

/// Settings › Store, Face ID copies on a device, Key diagnostics and
/// Remove (design.md 2.12). The store's part shows only on a set-up store,
/// not on the simulator's fixture.
struct StoreSettingsSections: View {
    let model: AppModel
    @Environment(\.storeControls) private var controls
    @Environment(\.dismiss) private var dismiss
    @State private var confirmingRemove = false
    @State private var removeProblem: String?
    @State private var showsDiagnostics = false

    var body: some View {
        if let controls {
            if !controls.copiesAvailable {
                Section {
                    Text("This iPhone can't keep Face ID copies. Every password needs your key.")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                        .accessibilityIdentifier("settings-no-copies")
                } header: {
                    PawSectionHeader("Face ID copies")
                }
            }
            Section {
                LabeledContent {
                    Text(controls.address)
                        .font(PawFont.footnote.monospaced())
                        .foregroundStyle(PawColor.labelSecondary)
                        .multilineTextAlignment(.trailing)
                        .textSelection(.enabled)
                } label: {
                    Text("Address")
                        .foregroundStyle(PawColor.label)
                }
                .accessibilityIdentifier("settings-address")
                ForEach(controls.hostFingerprints, id: \.self) { fingerprint in
                    VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                        Text("Server fingerprint")
                            .foregroundStyle(PawColor.label)
                        Text(fingerprint)
                            .font(PawFont.footnote.monospaced())
                            .foregroundStyle(PawColor.labelSecondary)
                            .textSelection(.enabled)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("settings-host-fingerprint")
                }
            } header: {
                PawSectionHeader("Store")
            }
        }
        GitHubSettingsSection()
        Section {
            Button("Key diagnostics") { showsDiagnostics = true }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("settings-key-diagnostics")
                .sheet(isPresented: $showsDiagnostics) {
                    // What the key session read when the key was plugged
                    // in: no PIN is asked here, and nothing is sent to the
                    // key. Opening a password is the test that uses the PIN,
                    // through the same session and its checks.
                    KeyDiagnosticsView(model: model)
                        .privacyCover()
                }
        } header: {
            PawSectionHeader("Key")
        } footer: {
            Text("Read from the key when it was plugged in.")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
        }
        if let controls {
            Section {
                if confirmingRemove {
                    InlineConfirmRow(
                        question: "Remove this store?", detail: removeWarning(controls),
                        confirmTitle: "Remove", cancelTitle: "Keep it",
                        identifier: "settings-remove", questionIdentifier: "settings-remove-warning",
                        onConfirm: { remove(controls) },
                        onCancel: { confirmingRemove = false })
                } else {
                    Button("Remove store from this iPhone\u{2026}") { confirmingRemove = true }
                        .foregroundStyle(PawColor.danger)
                        .accessibilityIdentifier("settings-remove-store")
                }
                if let removeProblem {
                    ErrorRow(kind: .error, problem: "The store wasn't removed", nextStep: removeProblem)
                }
            }
        }
    }

    private func removeWarning(_ controls: StoreControls) -> String {
        var text = "Deletes the store, its Face ID copies and your key's record from this iPhone. "
            + "The store on the server is not touched."
        if controls.unpushed == 1 {
            text += " 1 change not sent yet will be lost."
        } else if controls.unpushed > 1 {
            text += " \(controls.unpushed) changes not sent yet will be lost."
        }
        return text
    }

    private func remove(_ controls: StoreControls) {
        if let problem = controls.remove() {
            removeProblem = problem
        } else {
            dismiss()
        }
    }
}
