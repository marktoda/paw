import SwiftUI

/// Settings › Store › Who can open these passwords: each `.gpg-id` in the
/// store (the top, and any folder with its own), the keys it names, which
/// one is this key, and the passwords under it that are encrypted for a key
/// it no longer names. Read-only; nothing is decrypted. The names of those
/// passwords are shown here only, never logged.
struct RecipientsScreen: View {
    let writer: StoreWriter
    @State private var scopes: [RecipientScope]?
    @State private var problem: String?

    var body: some View {
        List {
            if let problem {
                ErrorRow(kind: .error, problem: "Couldn't read who can open them", nextStep: problem)
            } else if let scopes {
                ForEach(scopes, id: \.gpgId) { scope in
                    RecipientScopeSection(scope: scope, only: scopes.count == 1)
                }
            } else {
                ProgressView()
                    .frame(maxWidth: .infinity)
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Who can open them")
        .navigationBarTitleDisplayMode(.inline)
        .task { await load() }
    }

    private func load() async {
        let writer = writer
        // Reads the first bytes of every password file: off the main thread.
        let result = await Task.detached(priority: .userInitiated) {
            Result { try writer.recipientScopes() }
        }.value
        switch result {
        case .success(let found):
            scopes = found
            problem = nil
        case .failure(let error):
            problem = describeEditError(error)
        }
    }
}

private struct RecipientScopeSection: View {
    let scope: RecipientScope
    /// The store has one list of keys for everything.
    let only: Bool
    @State private var namesShown = false

    private var title: String {
        if scope.folder.isEmpty { return only ? "All passwords" : "Everywhere else" }
        return scope.folder
    }

    private var identifier: String {
        scope.folder.isEmpty ? "recipients-top" : "recipients-\(scope.folder)"
    }

    var body: some View {
        Section {
            ForEach(Array(scope.keys.enumerated()), id: \.offset) { _, key in
                RecipientKeyRow(key: key)
            }
            if !scope.otherKeyEntries.isEmpty {
                otherKeyRows
            }
        } header: {
            PawSectionHeader(title)
        } footer: {
            Text(footer)
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
                .accessibilityIdentifier("\(identifier)-footer")
        }
    }

    private var footer: String {
        var text = scope.entryCount == 1 ? "1 password." : "\(scope.entryCount) passwords."
        if scope.unsureEntries == 1 {
            text += " 1 may be for another key."
        } else if scope.unsureEntries > 1 {
            text += " \(scope.unsureEntries) may be for another key."
        }
        return text
    }

    @ViewBuilder private var otherKeyRows: some View {
        let count = scope.otherKeyEntries.count
        Button {
            withAnimation { namesShown.toggle() }
        } label: {
            HStack(spacing: PawMetrics.Spacing.m) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(PawColor.warning)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                    Text(count == 1 ? "1 password is for another key" : "\(count) passwords are for another key")
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.label)
                    Text("No key above opens \(count == 1 ? "it" : "them").")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
                Spacer(minLength: PawMetrics.Spacing.s)
                Image(systemName: PawSymbol.disclosure)
                    .font(PawFont.footnote.weight(.semibold))
                    .foregroundStyle(PawColor.labelTertiary)
                    .rotationEffect(.degrees(namesShown ? 90 : 0))
                    .accessibilityHidden(true)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityValue(namesShown ? "Names shown" : "")
        .accessibilityIdentifier("\(identifier)-other-keys")
        if namesShown {
            ForEach(scope.otherKeyEntries, id: \.self) { name in
                Text(name)
                    .font(PawFont.subheadline)
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("recipients-other-entry")
            }
        }
    }
}

private struct RecipientKeyRow: View {
    let key: ScopeKey

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
            Text(title)
                .font(PawFont.body)
                .foregroundStyle(PawColor.label)
            if let detail {
                Text(detail)
                    .font(PawFont.footnote)
                    .foregroundStyle(problem ? PawColor.warning : PawColor.labelSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let id = key.shortId {
                Text(KeyIdWords.grouped(id))
                    .font(PawFont.footnote.monospaced())
                    .foregroundStyle(PawColor.labelSecondary)
                    .textSelection(.enabled)
            }
        }
        .padding(.vertical, PawMetrics.Spacing.xxs)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(key.thisKey ? "recipients-this-key" : "recipients-key")
    }

    private var title: String {
        if key.thisKey { return "This key" }
        if let email = key.emails.first { return email }
        if let id = key.shortId { return "Key ending \(KeyIdWords.ending(id))" }
        return key.line
    }

    /// Only what changes what the person can do.
    private var detail: String? {
        switch key.status {
        case .ready:
            return nil
        case .missing:
            return "Not on this iPhone. Saving here needs it."
        case .ambiguous(let fingerprints):
            return "Matches \(fingerprints.count) keys on this iPhone."
        case .expired:
            return "Expired. Saving here needs a renewed key."
        case .refused(_, _, let reason):
            return "Can't be used: \(reason)."
        }
    }

    private var problem: Bool {
        if case .ready = key.status { return false }
        return true
    }
}
