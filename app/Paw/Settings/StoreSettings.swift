import SwiftUI

/// Settings › Store, Face ID copies on a device, and Key diagnostics
/// (design.md 2.12). The address shows only on a set-up store; the
/// server key and who can open the passwords show wherever the store can be
/// written, the simulator's fixture included. Remove is its own section,
/// `RemoveStoreSection`, last in Settings.
struct StoreSettingsSections: View {
    let model: AppModel
    @State private var showsDiagnostics = false

    /// The set-up store's address; `nil` for a fixture.
    private var setUpAddress: String? {
        model.storeSession.setUp == nil ? nil : (model.storeSession.syncing?.address ?? "")
    }

    var body: some View {
        if model.storeSession.setUp != nil {
            if model.storeSession.copies == nil {
                Section {
                    Text(Device.words("Not available on this iPhone. Every password needs your key."))
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                        .accessibilityIdentifier("settings-no-copies")
                } header: {
                    PawSectionHeader(Device.biometrics)
                }
            }
        }
        storeSection
        GitHubSettingsSection(address: setUpAddress, gitHub: model.storeSession.gitHub)
        Section {
            Button {
                showsDiagnostics = true
            } label: {
                HStack {
                    Text("Key diagnostics")
                        .foregroundStyle(PawColor.label)
                    Spacer(minLength: PawMetrics.Spacing.s)
                    Image(systemName: PawSymbol.disclosure)
                        .font(PawFont.footnote.weight(.semibold))
                        .foregroundStyle(PawColor.labelTertiary)
                        .accessibilityHidden(true)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
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
        }
    }

    /// Address, the server key, and who can open the passwords.
    @ViewBuilder private var storeSection: some View {
        let syncing = model.storeSession.syncing
        let writer = model.storeSession.edits?.writer
        if model.storeSession.setUp != nil || syncing != nil || writer != nil {
            Section {
                if let address = setUpAddress ?? syncing?.address {
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
                if let syncing {
                    ServerIdentityRows(sync: syncing.sync)
                }
                if let writer {
                    NavigationLink {
                        RecipientsScreen(writer: writer)
                    } label: {
                        Text("Who can open")
                            .foregroundStyle(PawColor.label)
                    }
                    .accessibilityIdentifier("settings-recipients")
                }
            } header: {
                PawSectionHeader("Store")
            }
        }
    }
}

/// Settings' last section on a set-up store: Remove, after an inline
/// confirmation that says what would be lost, counted as it is now.
struct RemoveStoreSection: View {
    let model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var confirmingRemove = false
    @State private var removeProblem: String?

    var body: some View {
        if let setUp = model.storeSession.setUp {
            Section {
                if confirmingRemove {
                    InlineConfirmRow(
                        question: "Remove this store?", detail: warning,
                        confirmTitle: "Remove", cancelTitle: "Keep it",
                        identifier: "settings-remove", questionIdentifier: "settings-remove-warning",
                        onConfirm: { remove(setUp) },
                        onCancel: { confirmingRemove = false })
                } else {
                    Button(Device.words("Remove store from this iPhone\u{2026}")) { confirmingRemove = true }
                        .foregroundStyle(PawColor.danger)
                        .accessibilityIdentifier("settings-remove-store")
                }
                if let removeProblem {
                    ErrorRow(kind: .error, problem: "The store wasn't removed", nextStep: removeProblem)
                }
            }
        }
    }

    /// What Remove deletes, and the changes it would lose as they are now:
    /// none where the store can't be written, unknown where its queue
    /// can't be read.
    var warning: String {
        let session = model.storeSession
        guard session.edits != nil else { return removeStoreWarning(unpushed: 0) }
        return removeStoreWarning(unpushed: session.syncing?.queue.unsentCount)
    }

    private func remove(_ setUp: SetUpStore) {
        if let problem = setUp.remove() {
            removeProblem = problem
        } else {
            dismiss()
        }
    }
}

/// Server keys forgotten in Settings since paw started, by host: so the
/// next check can say whether the key the server presents now is the one
/// the person trusted before. Memory only; gone when paw quits.
@MainActor
enum ForgottenServerKeys {
    static var byHost: [String: Set<String>] = [:]
}

/// The store's server: its name and why paw trusts it, one row. Tapping it
/// shows each server key, by type and SHA256 fingerprint, to compare with
/// the one the server publishes. A server key the person trusted (not one
/// that ships with paw, like GitHub's) can be forgotten; then the next
/// check shows the key the server presents now, says whether it is the one
/// forgotten, and asks before trusting a different one.
private struct ServerIdentityRows: View {
    let sync: StoreSync
    @State private var keys: ServerKeys?
    @State private var expanded = false
    @State private var confirmingForget = false
    /// The key the server presented, waiting for Trust.
    @State private var presented: ServerKey?
    /// A different key than the one forgotten: Trust asks again first.
    @State private var confirmingDifferent = false
    @State private var checking = false
    @State private var problem: (title: String, detail: String)?

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
                    Text(bundled ? "Key built into paw" : pinned.isEmpty ? "Not trusted yet" : "Trusted by you")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
                Spacer(minLength: PawMetrics.Spacing.s)
                Image(systemName: PawSymbol.disclosure)
                    .font(PawFont.footnote.weight(.semibold))
                    .foregroundStyle(PawColor.labelTertiary)
                    .rotationEffect(.degrees(expanded ? 90 : 0))
                    .accessibilityHidden(true)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityValue(expanded ? "Server key shown" : "")
        .accessibilityHint(expanded ? "" : "Shows the server key")
        .accessibilityIdentifier("settings-host-fingerprint")
        // Said in the row's problem if the pinned keys can't be read.
        .task { reload() }
        if expanded {
            if !pinned.isEmpty {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
                    ForEach(Array(pinned.enumerated()), id: \.offset) { _, key in
                        KeyFingerprintLines(key: key, identifier: "settings-host-key")
                    }
                    Text("Compare with the fingerprint your server publishes.")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("settings-host-fingerprints")
            }
            if !bundled {
                trustRows(host: shown?.host ?? "", pinned: pinned)
            }
            if let problem {
                ErrorRow(kind: .error, problem: problem.title, nextStep: problem.detail)
            }
        }
    }

    @ViewBuilder private func trustRows(host: String, pinned: [ServerKey]) -> some View {
        if !pinned.isEmpty {
            if confirmingForget {
                InlineConfirmRow(
                    question: "Forget the server key?",
                    detail: "Sync stops until you trust it again.",
                    confirmTitle: "Forget", cancelTitle: "Keep it",
                    identifier: "settings-forget-host",
                    onConfirm: { forget(host: host, pinned: pinned) },
                    onCancel: { confirmingForget = false })
            } else {
                Button("Forget server key\u{2026}") { confirmingForget = true }
                    .foregroundStyle(PawColor.danger)
                    .accessibilityIdentifier("settings-forget-host-key")
            }
        } else if let presented {
            let before = ForgottenServerKeys.byHost[host] ?? []
            let differs = !before.isEmpty && !before.contains(presented.fingerprint)
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
                if differs {
                    Label("Different from the key you trusted before", systemImage: "exclamationmark.triangle.fill")
                        .font(PawFont.subheadline.weight(.semibold))
                        .foregroundStyle(PawColor.danger)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityElement(children: .ignore)
                        .accessibilityLabel("Different from the key you trusted before")
                        .accessibilityIdentifier("settings-presented-key-differs")
                } else if !before.isEmpty {
                    Text("Same key as before")
                        .font(PawFont.subheadline)
                        .foregroundStyle(PawColor.labelSecondary)
                        .accessibilityIdentifier("settings-presented-key-same")
                }
                KeyFingerprintLines(key: presented, identifier: "settings-presented-key")
                Text("Compare with the fingerprint your server publishes.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("settings-presented-host-key")
            if differs && confirmingDifferent {
                InlineConfirmRow(
                    question: "Trust a different key?",
                    detail: "Only if your server's key was changed on purpose.",
                    confirmTitle: "Trust", cancelTitle: "Cancel",
                    identifier: "settings-trust-different",
                    onConfirm: { trust(presented, host: host) },
                    onCancel: { confirmingDifferent = false })
            } else {
                Button(differs ? "Trust the new key\u{2026}" : "Trust this key") {
                    if differs {
                        confirmingDifferent = true
                    } else {
                        trust(presented, host: host)
                    }
                }
                .foregroundStyle(differs ? PawColor.danger : PawColor.label)
                .accessibilityIdentifier("settings-trust-host")
                Button("Cancel") { self.presented = nil }
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("settings-trust-host-cancel")
            }
        } else {
            Button(checking ? "Checking\u{2026}" : "Check the server key") { check() }
                .foregroundStyle(PawColor.label)
                .disabled(checking)
                .accessibilityIdentifier("settings-check-host")
        }
    }

    private func reload() {
        do {
            keys = try sync.serverKeys()
        } catch {
            problem = ("Couldn't read the server key", describe(error))
        }
    }

    private func forget(host: String, pinned: [ServerKey]) {
        confirmingForget = false
        do {
            try sync.forgetHostKey()
            ForgottenServerKeys.byHost[host, default: []].formUnion(pinned.map(\.fingerprint))
            problem = nil
        } catch {
            problem = ("The server key wasn't forgotten", describe(error))
        }
        reload()
    }

    /// Connects without the key and without logging in: network only.
    private func check() {
        checking = true
        problem = nil
        confirmingDifferent = false
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
                problem = ("Couldn't reach the server", describe(error))
            }
        }
    }

    private func trust(_ key: ServerKey, host: String) {
        let sync = sync
        Task {
            let result = await Task.detached(priority: .userInitiated) {
                Result { try sync.trustServerKey(fingerprint: key.fingerprint) }
            }.value
            presented = nil
            confirmingDifferent = false
            if case .failure(let error) = result {
                problem = ("The server key wasn't trusted", describe(error))
            } else {
                ForgottenServerKeys.byHost[host] = nil
                problem = nil
            }
            reload()
        }
    }
}

/// One server key: "Ed25519 key", then its whole fingerprint beneath.
private struct KeyFingerprintLines: View {
    let key: ServerKey
    let identifier: String

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
            Text("\(key.keyType) key")
                .font(PawFont.footnote.weight(.semibold))
                .foregroundStyle(PawColor.label)
            FingerprintText(fingerprint: key.fingerprint)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(identifier)
    }
}
