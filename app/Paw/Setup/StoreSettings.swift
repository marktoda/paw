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
                ServerIdentityRows(address: controls.address, fingerprints: controls.hostFingerprints)
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
                    // in, the touch timeout check (a key task, through the
                    // same session and its checks) and the key log.
                    KeyDiagnosticsView(model: model)
                        .privacyCover()
                }
        } header: {
            PawSectionHeader("Key")
        } footer: {
            Text("What your key said when plugged in, a touch timeout check, and the key log.")
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

/// The store's server: its name and why paw trusts it, one row. Tapping it
/// shows the fingerprints in place, to compare with the server's own.
private struct ServerIdentityRows: View {
    let address: String
    let fingerprints: [String]
    @State private var expanded = false

    private var reading: AddressReading? { try? readStoreAddress(text: address) }

    var body: some View {
        let reading = reading
        let bundled = reading?.hostKeyBundled ?? false
        Button {
            withAnimation { expanded.toggle() }
        } label: {
            HStack(spacing: PawMetrics.Spacing.m) {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                    Text(reading?.host ?? "Server")
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.label)
                    Text(bundled ? "Identity built into paw" : "Trusted by you")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
                Spacer(minLength: PawMetrics.Spacing.s)
                Image(systemName: "chevron.right")
                    .font(PawFont.footnote.weight(.semibold))
                    .foregroundStyle(PawColor.labelTertiary)
                    .rotationEffect(.degrees(expanded ? 90 : 0))
                    .accessibilityHidden(true)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityValue(expanded ? "Fingerprints shown" : "")
        .accessibilityHint(expanded ? "" : "Shows the server's fingerprints")
        .accessibilityIdentifier("settings-host-fingerprint")
        if expanded {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
                ForEach(Array(fingerprints.enumerated()), id: \.offset) { index, fingerprint in
                    VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                        if let type = Self.keyType(index, of: fingerprints.count, bundled: bundled) {
                            Text(type)
                                .font(PawFont.footnote.weight(.semibold))
                                .foregroundStyle(PawColor.label)
                        }
                        // One line, shrunk to fit: wrapped, iOS put a
                        // hyphen inside it that is not part of it.
                        Text(fingerprint)
                            .font(PawFont.footnote.monospaced())
                            .foregroundStyle(PawColor.labelSecondary)
                            .lineLimit(1)
                            .minimumScaleFactor(0.5)
                            .textSelection(.enabled)
                    }
                    .accessibilityElement(children: .combine)
                }
                Text("Compare with your server's published fingerprints.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("settings-host-fingerprints")
        }
    }

    /// The core gives fingerprints without their types. Every bundled host
    /// ships Ed25519, ECDSA and RSA keys in that order (core
    /// `sync/hosts.rs`); a key the person confirmed has no type to show.
    static func keyType(_ index: Int, of count: Int, bundled: Bool) -> String? {
        let types = ["Ed25519", "ECDSA", "RSA"]
        guard bundled, count == types.count else { return nil }
        return types[index]
    }
}
