import SwiftUI

/// Settings › Store, Face ID copies on a device, Key diagnostics and
/// Remove (design.md 2.12). The store's part shows only on a set-up store,
/// not on the simulator's fixture.
struct StoreSettingsSections: View {
    @Environment(\.storeControls) private var controls
    @Environment(\.dismiss) private var dismiss
    @State private var confirmingRemove = false
    @State private var removeProblem: String?
    @State private var showsDiagnostics = false

    var body: some View {
        if let controls {
            if !controls.copiesAvailable {
                Section {
                    Text("Face ID copies can't be kept on this iPhone, so every password opens with your key.")
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
        Section {
            Button("Key diagnostics") { showsDiagnostics = true }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("settings-key-diagnostics")
                .sheet(isPresented: $showsDiagnostics) {
                    SelfTestView(host: "app")
                        .privacyCover()
                }
        } header: {
            PawSectionHeader("Key")
        } footer: {
            Text("Reads your key and decrypts a test message, to check the key and this iPhone work together.")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
        }
        if let controls {
            Section {
                if confirmingRemove {
                    Text(removeWarning(controls))
                        .font(PawFont.subheadline)
                        .foregroundStyle(PawColor.label)
                        .accessibilityIdentifier("settings-remove-warning")
                    Button("Remove") { remove(controls) }
                        .font(PawFont.body.weight(.semibold))
                        .foregroundStyle(PawColor.danger)
                        .accessibilityIdentifier("settings-remove-confirm")
                    Button("Keep it") { confirmingRemove = false }
                        .foregroundStyle(PawColor.label)
                        .accessibilityIdentifier("settings-remove-cancel")
                } else {
                    Button("Remove this store from this iPhone\u{2026}") { confirmingRemove = true }
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
        var text = "This deletes the downloaded store, its Face ID copies and this iPhone's record of your key. "
            + "Your store on the server is not touched."
        if controls.unpushed == 1 {
            text += " 1 change that is only on this iPhone will be lost."
        } else if controls.unpushed > 1 {
            text += " \(controls.unpushed) changes that are only on this iPhone will be lost."
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
