import SwiftUI

/// The screen for a `Route.sync`.
struct SyncRouteScreen: View {
    let model: AppModel
    let route: SyncRoute

    var body: some View {
        if let edits = model.edits, let syncing = edits.syncing {
            switch route {
            case .heldBack:
                HeldBackScreen(model: model, syncing: syncing)
            case .refusedRemote:
                RefusedRemoteScreen(model: model, syncing: syncing)
            case .replaced:
                ReplacedScreen(model: model, syncing: syncing)
            case .review(let folder):
                RecipientsReviewScreen(writer: edits.writer, folder: folder) {
                    model.path.removeAll { $0 == .sync(.review(folder: folder)) }
                    syncing.recomputeLocal()
                }
            }
        } else {
            EmptyState(
                symbol: PawSymbol.synced, title: "Nothing to sync",
                message: "This store has no git remote paw can reach.")
        }
    }
}

/// A plain paragraph at the top of a sync screen, on the background.
private struct SyncIntro: View {
    let headline: String
    let text: String
    var identifier = "sync-intro"

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
            Text(headline)
                .font(PawFont.headline)
                .foregroundStyle(PawColor.label)
                .accessibilityAddTraits(.isHeader)
            Text(text)
                .font(PawFont.subheadline)
                .foregroundStyle(PawColor.labelSecondary)
        }
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(identifier)
        .listRowBackground(Color.clear)
        .listRowInsets(EdgeInsets(
            top: PawMetrics.Spacing.s, leading: PawMetrics.Spacing.l,
            bottom: PawMetrics.Spacing.s, trailing: PawMetrics.Spacing.l))
    }
}

/// Waiting changes paw will not send as they are: each one's ciphertext is
/// for other keys than its folder's list says now. For each, who it is
/// encrypted for and who it should be for, and the ways out. Sending it as
/// it is is never offered.
struct HeldBackScreen: View {
    let model: AppModel
    let syncing: SyncModel
    @State private var confirming: String?

    var body: some View {
        List {
            Section {
                SyncIntro(
                    headline: "These changes weren't sent",
                    text: "Who can open their folder changed after you saved them, so they are encrypted for the "
                        + "old list of keys. paw won't send them like that: the old list could open them, and the "
                        + "new one couldn't. Review the new list, then open each one and save it again, or discard it.")
            }
            if syncing.heldBack.isEmpty {
                Section {
                    EmptyState(
                        symbol: PawSymbol.synced, title: "Nothing held back",
                        message: "Every waiting change can be sent.")
                        .accessibilityElement(children: .combine)
                        .accessibilityIdentifier("held-back-empty")
                        .listRowBackground(Color.clear)
                }
            }
            ForEach(syncing.heldBack, id: \.id) { item in
                section(item)
            }
        }
        .listStyle(.insetGrouped)
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .navigationTitle("Held back")
        .navigationBarTitleDisplayMode(.large)
        .onAppear { syncing.recomputeLocal() }
    }

    private func section(_ item: HeldBack) -> some View {
        let entry = EntryRef(path: item.entry)
        let folder = item.gpgId.split(separator: "/").dropLast().joined(separator: "/")
        let place = folder.isEmpty ? "your store" : folder
        return Section {
            detailRow(
                "Encrypted for",
                item.encryptedFor.isEmpty ? "Nobody paw can name" : item.encryptedFor.joined(separator: "\n"),
                prose: item.encryptedFor.isEmpty,
                identifier: "held-back-encrypted-for-\(entry.name)")
            detailRow(
                item.gpgId.isEmpty ? "Should be for" : "Should be for (\(item.gpgId))",
                item.recipients.isEmpty
                    ? "paw can't read who this folder is for."
                    : item.recipients.map(RecipientsReviewScreen.grouped).joined(separator: "\n"),
                prose: item.recipients.isEmpty,
                note: item.accepted ? nil : "Not accepted on this iPhone yet.",
                identifier: "held-back-should-be-for-\(entry.name)")
            if !item.accepted {
                actionButton("Review who can open \(place)", identifier: "held-back-review-\(entry.name)") {
                    model.path.append(.sync(.review(folder: folder)))
                }
            }
            if item.accepted {
                Button {
                    model.openWithKey(entry)
                } label: {
                    KeyActionLabel("Open and save again")
                        .font(PawFont.body.weight(.semibold))
                        .foregroundStyle(PawColor.label)
                        .frame(maxWidth: .infinity, minHeight: PawMetrics.tapMin, alignment: .leading)
                }
                .accessibilityIdentifier("held-back-open-\(entry.name)")
                .pawListRow()
            }
            if confirming == item.id {
                InlineConfirmRow(
                    question: "Discard this change to \(entry.name)?",
                    detail: "It is taken back on this iPhone and never sent. Later changes are kept.",
                    confirmTitle: "Discard", cancelTitle: "Keep",
                    identifier: "held-back-discard-\(entry.name)",
                    onConfirm: {
                        confirming = nil
                        model.discardChange(item.id)
                        syncing.recomputeLocal()
                    },
                    onCancel: { confirming = nil })
                    .pawListRow()
            } else {
                Button {
                    confirming = item.id
                } label: {
                    Text("Discard this change")
                        .font(PawFont.body.weight(.semibold))
                        .foregroundStyle(PawColor.danger)
                        .frame(maxWidth: .infinity, minHeight: PawMetrics.tapMin, alignment: .leading)
                }
                .accessibilityIdentifier("held-back-discard-button-\(entry.name)")
                .pawListRow()
            }
        } header: {
            PawSectionHeader(entry.folder.isEmpty ? entry.name : "\(entry.name) \u{00B7} \(entry.folder)")
                .accessibilityIdentifier("held-back-\(entry.name)")
        } footer: {
            if !item.accepted {
                Text("Once you've accepted the new list, open it with your key and save it again: saving encrypts it for everyone on the list. Then discard this older change.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
    }

    private func detailRow(_ title: String, _ value: String, prose: Bool = false, note: String? = nil,
                           identifier: String) -> some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
            Text(title)
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
            Text(value)
                .font(prose ? PawFont.footnote : PawFont.footnote.monospaced())
                .foregroundStyle(PawColor.label)
                .textSelection(.enabled)
            if let note {
                Text(note)
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.warning)
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .padding(.vertical, PawMetrics.Spacing.s)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(identifier)
        .pawListRow()
    }

    private func actionButton(_ title: String, identifier: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(PawFont.body.weight(.semibold))
                .foregroundStyle(PawColor.label)
                .frame(maxWidth: .infinity, minHeight: PawMetrics.tapMin, alignment: .leading)
        }
        .accessibilityIdentifier(identifier)
        .pawListRow()
    }
}

/// The git remote holds something paw will not check out: what and why,
/// and that it has to be fixed from another computer. Nothing here was
/// changed and the waiting changes stay queued.
struct RefusedRemoteScreen: View {
    let model: AppModel
    let syncing: SyncModel

    private var unpushed: Int {
        model.edits?.unpushed.count ?? 0
    }

    var body: some View {
        List {
            Section {
                SyncIntro(
                    headline: "Your git remote contains something paw won't check out",
                    text: "Nothing on this iPhone was changed. \(SyncFailure.waiting(unpushed)) "
                        + "Fix these from another computer: rename or remove them, then push. "
                        + "Until then paw neither checks for changes nor sends.")
            }
            if syncing.refused.isEmpty {
                Section {
                    EmptyState(
                        symbol: PawSymbol.synced, title: "Nothing refused",
                        message: "What paw last fetched can be checked out. Check for changes to sync again.")
                        .listRowBackground(Color.clear)
                }
            } else {
                Section {
                    ForEach(syncing.refused, id: \.path) { refused in
                        VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                            Text(refused.path)
                                .font(PawFont.body.monospaced())
                                .foregroundStyle(PawColor.label)
                                .textSelection(.enabled)
                            Text(refused.reason.capitalizedFirst)
                                .font(PawFont.footnote)
                                .foregroundStyle(PawColor.labelSecondary)
                        }
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.vertical, PawMetrics.Spacing.s)
                        .accessibilityElement(children: .combine)
                        .accessibilityIdentifier("refused-path")
                        .pawListRow()
                    }
                } header: {
                    PawSectionHeader(syncing.refused.count == 1 ? "1 name" : "\(syncing.refused.count) names")
                }
            }
            Section {
                Button {
                    syncing.start(.refresh, app: model)
                } label: {
                    KeyActionLabel("Check again")
                }
                .buttonStyle(.pawSecondary)
                .accessibilityIdentifier("refused-check-again")
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            } footer: {
                Text("Once it's fixed on the git remote, checking again brings in the fix. It needs your key.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
        .listStyle(.insetGrouped)
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .navigationTitle("Can't check out")
        .navigationBarTitleDisplayMode(.inline)
    }
}

/// The git remote's history was replaced (a force-push, or the repository
/// made again). Says what that means and what is at stake, then two
/// choices, the one that loses changes behind an inline confirm, and
/// "Not now". Taking it needs no key: it is all local.
struct ReplacedScreen: View {
    let model: AppModel
    let syncing: SyncModel
    @State private var confirmingDiscard = false

    private var unpushed: Int {
        model.edits?.unpushed.count ?? 0
    }

    private var changes: String {
        unpushed == 1 ? "1 change" : "\(unpushed) changes"
    }

    var body: some View {
        List {
            Section {
                SyncIntro(
                    headline: "The store on your git remote was replaced",
                    text: "Its history no longer matches this iPhone's. Nothing here has been changed. "
                        + "This happens when someone force-pushes or makes the repository again. "
                        + "If you didn't expect it, ask whoever looks after the store before you go on.",
                    identifier: "replaced-intro")
            }
            if unpushed > 0 {
                Section {
                    ForEach(model.edits?.unpushed.prefix(5) ?? [], id: \.id) { item in
                        Text(UnpushedScreen.title(item))
                            .font(PawFont.body)
                            .foregroundStyle(PawColor.label)
                            .frame(minHeight: PawMetrics.rowOneLine, alignment: .leading)
                            .pawListRow()
                    }
                    if unpushed > 5 {
                        Text("and \(unpushed - 5) more")
                            .font(PawFont.footnote)
                            .foregroundStyle(PawColor.labelSecondary)
                            .pawListRow()
                    }
                } header: {
                    PawSectionHeader("Your \(changes) not sent yet")
                        .accessibilityIdentifier("replaced-unpushed")
                } footer: {
                    Text("Keeping them adds them on top of the new history, as if you had made them after it. "
                        + "If the same password changed there too, both versions are kept.")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
            }
            if let problem = syncing.acceptProblem {
                Section {
                    ErrorRow(kind: .error, problem: "Nothing was changed.", nextStep: problem)
                        .pawListRow()
                }
            }
            Section {
                Button {
                    confirmingDiscard = false
                    syncing.acceptReplaced(keepChanges: true, app: model)
                } label: {
                    Text(unpushed > 0 ? "Use the git remote's store and keep my \(changes)" : "Use the git remote's store")
                }
                .buttonStyle(.pawPrimary)
                .disabled(syncing.accepting)
                .accessibilityIdentifier("replaced-keep")
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            }
            if unpushed > 0 {
                Section {
                    if confirmingDiscard {
                        InlineConfirmRow(
                            question: "Discard \(changes)?",
                            detail: "Your \(unpushed == 1 ? "unpushed change" : "\(unpushed) unpushed changes") will be lost. "
                                + "This can't be undone.",
                            confirmTitle: "Discard and use it", cancelTitle: "Keep them",
                            identifier: "replaced-discard",
                            onConfirm: {
                                confirmingDiscard = false
                                syncing.acceptReplaced(keepChanges: false, app: model)
                            },
                            onCancel: { confirmingDiscard = false })
                            .pawListRow()
                    } else {
                        Button {
                            confirmingDiscard = true
                        } label: {
                            Text("Use the git remote's store and discard my changes")
                                .font(PawFont.body.weight(.semibold))
                                .foregroundStyle(PawColor.danger)
                                .frame(maxWidth: .infinity, minHeight: PawMetrics.tapMin, alignment: .leading)
                        }
                        .disabled(syncing.accepting)
                        .accessibilityIdentifier("replaced-discard-button")
                        .pawListRow()
                    }
                }
            }
            Section {
                Button("Not now") {
                    syncing.notNow(app: model)
                }
                .buttonStyle(.pawSecondary)
                .accessibilityIdentifier("replaced-not-now")
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            } footer: {
                Text("Until you choose, nothing is sent or brought in, and the pill on Home says sync is paused.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
        .listStyle(.insetGrouped)
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .navigationTitle("History replaced")
        .navigationBarTitleDisplayMode(.inline)
    }
}
