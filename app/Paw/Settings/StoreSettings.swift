import SwiftUI

/// Settings › Store, the GitHub sign-in and Key diagnostics (design.md
/// 2.12).
struct StoreSettingsSections: View {
    let model: AppModel
    @State private var showsDiagnostics = false

    /// The set-up store's address; `nil` for a fixture, and while sync is
    /// off.
    private var setUpAddress: String? {
        model.storeSession.setUp == nil ? nil : model.storeSession.syncing?.address
    }

    var body: some View {
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
            .pawSheet(isPresented: $showsDiagnostics) {
                KeyDiagnosticsView(model: model)
                    .privacyCover()
            }
        } header: {
            PawSectionHeader("Key")
        }
    }

    @ViewBuilder private var storeSection: some View {
        let syncing = model.storeSession.syncing
        let writer = model.storeSession.edits?.writer
        if model.storeSession.setUp != nil || syncing != nil || writer != nil {
            Section {
                if let address = syncing?.address {
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
                } else if let problem = model.storeSession.syncProblem {
                    ErrorRow(kind: .warning, problem: SyncWords.off, nextStep: describe(problem))
                }
                if let syncing {
                    ServerIdentityRows(serverKeys: syncing.serverKeys)
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
                        identifier: "settings-remove",
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

/// The store's server: its name and why paw trusts it, one row. Tapping it
/// shows each server key, by type and SHA256 fingerprint, to compare with
/// the one the server publishes. A server key the person trusted (not one
/// that ships with paw, like GitHub's) can be forgotten; then the next
/// check shows the key the server presents now, says whether it is the one
/// forgotten, and asks before trusting a different one.
private struct ServerIdentityRows: View {
    let serverKeys: ServerKeysModel
    @State private var expanded = false
    @State private var confirmingForget = false
    /// A different key than the one forgotten: Trust asks again first.
    @State private var confirmingDifferent = false

    var body: some View {
        let shown = serverKeys.keys
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
        .task { serverKeys.read() }
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
                trustRows(pinned: pinned)
            }
            if let problem = serverKeys.problem {
                ErrorRow(kind: .error, problem: problem.title, nextStep: problem.detail)
            }
        }
    }

    @ViewBuilder private func trustRows(pinned: [ServerKey]) -> some View {
        if !pinned.isEmpty {
            if confirmingForget {
                InlineConfirmRow(
                    question: "Forget the server key?",
                    detail: "Sync stops until you trust it again.",
                    confirmTitle: "Forget", cancelTitle: "Keep it",
                    identifier: "settings-forget-host",
                    onConfirm: {
                        confirmingForget = false
                        serverKeys.forget()
                    },
                    onCancel: { confirmingForget = false })
            } else {
                Button("Forget server key\u{2026}") { confirmingForget = true }
                    .foregroundStyle(PawColor.danger)
                    .accessibilityIdentifier("settings-forget-host-key")
            }
        } else if let presented = serverKeys.presented {
            let differs = serverKeys.presentedDiffers
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
                if differs {
                    Label("Different from the key you trusted before", systemImage: "exclamationmark.triangle.fill")
                        .font(PawFont.subheadline.weight(.semibold))
                        .foregroundStyle(PawColor.danger)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityElement(children: .ignore)
                        .accessibilityLabel("Different from the key you trusted before")
                        .accessibilityIdentifier("settings-presented-key-differs")
                } else if serverKeys.forgottenBefore {
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
                    onConfirm: { serverKeys.trust(presented) },
                    onCancel: { confirmingDifferent = false })
            } else {
                Button(differs ? "Trust the new key\u{2026}" : "Trust this key") {
                    if differs {
                        confirmingDifferent = true
                    } else {
                        serverKeys.trust(presented)
                    }
                }
                .foregroundStyle(differs ? PawColor.danger : PawColor.label)
                .accessibilityIdentifier("settings-trust-host")
                Button("Cancel") { serverKeys.dropPresented() }
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("settings-trust-host-cancel")
            }
        } else {
            Button(serverKeys.checking ? "Checking\u{2026}" : "Check the server key") {
                confirmingDifferent = false
                serverKeys.check()
            }
            .foregroundStyle(PawColor.label)
            .disabled(serverKeys.checking)
            .accessibilityIdentifier("settings-check-host")
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
