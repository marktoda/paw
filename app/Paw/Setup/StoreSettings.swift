import SwiftUI

/// Settings › Store, Face ID copies on a device, Key diagnostics and
/// Remove (design.md 2.12). Address and Remove show only on a set-up
/// store; the server's keys and who can open the passwords show wherever
/// the store can be written, the simulator's fixture included.
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
        }
        storeSection
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

    /// Address, the server and its keys, and who can open the passwords.
    @ViewBuilder private var storeSection: some View {
        let sync = model.edits?.sync
        let writer = model.edits?.writer
        if controls != nil || sync != nil || writer != nil {
            Section {
                if let address = controls?.address ?? sync?.address() {
                    LabeledContent {
                        Text(address)
                            .font(PawFont.footnote.monospaced())
                            .foregroundStyle(PawColor.labelSecondary)
                            .multilineTextAlignment(.trailing)
                            .textSelection(.enabled)
                    } label: {
                        Text("Address")
                            .foregroundStyle(PawColor.label)
                    }
                    .accessibilityIdentifier("settings-address")
                }
                if let sync {
                    ServerIdentityRows(sync: sync)
                }
                if let writer {
                    NavigationLink {
                        RecipientsScreen(writer: writer)
                    } label: {
                        Text("Who can open these passwords")
                            .foregroundStyle(PawColor.label)
                    }
                    .accessibilityIdentifier("settings-recipients")
                }
            } header: {
                PawSectionHeader("Store")
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
/// shows each key the server must present, by type and SHA256 fingerprint,
/// to compare with the server's own. A server the person trusted (not one
/// whose keys ship with paw, like GitHub's) can be forgotten; then the
/// next check shows the key it presents now, to trust again.
private struct ServerIdentityRows: View {
    let sync: StoreSync
    @State private var keys: ServerKeys?
    @State private var expanded = false
    @State private var confirmingForget = false
    /// The key the server presented, waiting for "Trust this server".
    @State private var presented: ServerKey?
    @State private var checking = false
    @State private var problem: String?

    var body: some View {
        let shown = self.keys ?? (try? sync.serverKeys())
        let bundled = shown?.bundled ?? false
        let pinned = shown?.keys ?? []
        Button {
            withAnimation { expanded.toggle() }
        } label: {
            HStack(spacing: PawMetrics.Spacing.m) {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                    Text(shown?.host ?? "Server")
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.label)
                    Text(bundled ? "Identity built into paw" : pinned.isEmpty ? "Not trusted yet" : "Trusted by you")
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
                ForEach(Array(pinned.enumerated()), id: \.offset) { _, key in
                    KeyFingerprintLines(key: key, identifier: "settings-host-key")
                }
                Text(pinned.isEmpty
                     ? "paw asks you to check the server's key before it connects again."
                     : "paw only connects to a server that shows one of these keys. "
                        + "Compare them with your server's published fingerprints.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("settings-host-fingerprints")
            if !bundled {
                trustRows(pinned: pinned)
            }
            if let problem {
                ErrorRow(kind: .error, problem: "The server's key wasn't changed", nextStep: problem)
            }
        }
    }

    @ViewBuilder private func trustRows(pinned: [ServerKey]) -> some View {
        if !pinned.isEmpty {
            if confirmingForget {
                InlineConfirmRow(
                    question: "Forget this server's key?",
                    detail: "Sending and checking for changes wait until you check its key again.",
                    confirmTitle: "Forget", cancelTitle: "Keep it",
                    identifier: "settings-forget-host",
                    onConfirm: forget,
                    onCancel: { confirmingForget = false })
            } else {
                Button("Forget server key\u{2026}") { confirmingForget = true }
                    .foregroundStyle(PawColor.danger)
                    .accessibilityIdentifier("settings-forget-host-key")
            }
        } else if let presented {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
                KeyFingerprintLines(key: presented, identifier: "settings-presented-key")
                Text("Compare this with what your server's admin gave you.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("settings-presented-host-key")
            Button("Trust this server") { trust(presented) }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("settings-trust-host")
            Button("Cancel") { self.presented = nil }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("settings-trust-host-cancel")
        } else {
            Button(checking ? "Checking\u{2026}" : "Check the server's key") { check() }
                .foregroundStyle(PawColor.label)
                .disabled(checking)
                .accessibilityIdentifier("settings-check-host")
        }
    }

    private func reload() {
        keys = try? sync.serverKeys()
    }

    private func forget() {
        confirmingForget = false
        do {
            try sync.forgetHostKey()
            problem = nil
        } catch {
            problem = describeEditError(error)
        }
        reload()
    }

    /// Connects without the key and without logging in: network only.
    private func check() {
        checking = true
        problem = nil
        let sync = sync
        Task {
            let result = await Task.detached(priority: .userInitiated) {
                Result { try sync.checkServerKey() }
            }.value
            checking = false
            switch result {
            case .success(let key):
                presented = key
            case .failure(let error):
                problem = describeEditError(error)
            }
        }
    }

    private func trust(_ key: ServerKey) {
        let sync = sync
        Task {
            let result = await Task.detached(priority: .userInitiated) {
                Result { try sync.trustServerKey(fingerprint: key.fingerprint) }
            }.value
            presented = nil
            if case .failure(let error) = result {
                problem = describeEditError(error)
            } else {
                problem = nil
            }
            reload()
        }
    }
}

/// One host key: its type, then its fingerprint on one line.
private struct KeyFingerprintLines: View {
    let key: ServerKey
    let identifier: String

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
            Text(key.keyType)
                .font(PawFont.footnote.weight(.semibold))
                .foregroundStyle(PawColor.label)
            // One line, shrunk to fit: wrapped, iOS put a hyphen inside it
            // that is not part of it.
            Text(key.fingerprint)
                .font(PawFont.footnote.monospaced())
                .foregroundStyle(PawColor.labelSecondary)
                .lineLimit(1)
                .minimumScaleFactor(0.5)
                .textSelection(.enabled)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(identifier)
    }
}
