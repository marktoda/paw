import SwiftUI

struct SyncRouteScreen: View {
    let model: AppModel
    let route: SyncRoute

    var body: some View {
        if let edits = model.storeSession.edits, let syncing = model.storeSession.syncing {
            switch route {
            case .heldBack:
                HeldBackScreen(model: model, syncing: syncing)
            case .refusedRemote:
                RefusedRemoteScreen(model: model, syncing: syncing)
            case .replaced:
                ReplacedScreen(model: model, syncing: syncing)
            case .review(let folder):
                RecipientsReviewScreen(edits: edits, folder: folder) {
                    model.leave(.sync(.review(folder: folder)))
                }
            }
        } else {
            EmptyState(
                symbol: PawSymbol.synced, title: "Nothing to sync",
                message: "This store has no address to sync with.")
        }
    }
}

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
            top: PawMetrics.Spacing.s, leading: PawMetrics.Spacing.xs,
            bottom: PawMetrics.Spacing.s, trailing: PawMetrics.Spacing.xs))
    }
}

/// Waiting changes paw will not send as they are: each one's ciphertext is
/// for other keys than its folder's list says now. Sending it as it is is
/// never offered.
struct HeldBackScreen: View {
    let model: AppModel
    let syncing: SyncModel
    @State private var confirming: String?

    var body: some View {
        List {
            Section {
                SyncIntro(
                    headline: "These changes weren't sent",
                    text: "Who can open the folder changed after you saved, so these are encrypted for the old "
                        + "list of keys. Review the new list, then re-encrypt them.")
            }
            if syncing.canReencrypt {
                Section {
                    ReencryptButton(syncing: syncing, identifier: "held-back-reencrypt")
                        .listRowInsets(EdgeInsets())
                        .listRowBackground(Color.clear)
                }
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
    }

    private func heldBackNote(_ item: HeldBack) -> String? {
        switch item.list {
        case .accepted: syncing.reencryptLeft[item.entry].flatMap(reencryptWords)
        case .notAccepted: Device.words("Not accepted on this iPhone yet.")
        case .unreadable: nil
        }
    }

    private func section(_ item: HeldBack) -> some View {
        let entry = EntryRef(path: item.entry)
        let folder = item.gpgId.map { gpgIdFolder(path: $0) } ?? ""
        let place = folder.isEmpty ? "your store" : folder
        return Section {
            detailRow(
                "Encrypted for",
                item.encryptedFor.isEmpty ? "Keys paw can't name" : item.encryptedFor.map(recipientWords).joined(separator: "\n"),
                prose: item.encryptedFor.isEmpty,
                identifier: "held-back-encrypted-for-\(entry.name)")
            let listUnread = item.list == .unreadable || item.recipients.isEmpty
            detailRow(
                "Should be for",
                listUnread
                    ? "Can't read this folder's list of keys."
                    : item.recipients.map(RecipientLineWords.grouped).joined(separator: "\n"),
                prose: listUnread,
                note: heldBackNote(item),
                identifier: "held-back-should-be-for-\(entry.name)")
            // A list that can't be read has nothing to review.
            if item.list == .notAccepted, item.gpgId != nil {
                actionButton("Review who can open \(place)", identifier: "held-back-review-\(entry.name)") {
                    model.show(.sync(.review(folder: folder)))
                }
            }
            if confirming == item.id {
                InlineConfirmRow(
                    question: "Discard this change to \(entry.name)?",
                    detail: Device.words("It's undone on this iPhone and never sent."),
                    confirmTitle: "Discard", cancelTitle: "Keep",
                    identifier: "held-back-discard-\(entry.name)",
                    onConfirm: {
                        confirming = nil
                        model.storeSession.edits?.discardChange(item.id)
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

    var body: some View {
        List {
            Section {
                SyncIntro(
                    headline: SyncFailure.refusedRemoteProblem,
                    text: sentences(
                        "Rename or remove them on a computer. Sync is paused until then.",
                        SyncFailure.waiting(syncing.unpushed)))
            }
            if syncing.refused.isEmpty {
                Section {
                    EmptyState(
                        symbol: PawSymbol.synced, title: "Nothing refused",
                        message: "Check for changes to sync again.")
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
                            Text(refusedWords(refused).capitalizedFirst)
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
                    PawSectionHeader(counted(syncing.refused.count, "name", "names"))
                }
            }
            Section {
                Button {
                    syncing.check()
                } label: {
                    if syncing.checkNeedsKey {
                        KeyActionLabel("Check again")
                    } else {
                        Text("Check again")
                    }
                }
                .buttonStyle(.pawSecondary)
                .accessibilityIdentifier("refused-check-again")
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            }
        }
        .listStyle(.insetGrouped)
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .navigationTitle("Can't download")
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

    private var changes: String {
        changeCount(syncing.unpushed)
    }

    var body: some View {
        List {
            Section {
                SyncIntro(
                    headline: "The store on \(syncing.host) was replaced",
                    text: Device.words("Its history no longer matches this iPhone's. ")
                        + "Not expected? Ask whoever looks after the store first.",
                    identifier: "replaced-intro")
            }
            if syncing.unpushed > 0 {
                Section {
                    ForEach(syncing.queue.unpushed.prefix(5), id: \.id) { item in
                        Text(UnpushedScreen.title(item))
                            .font(PawFont.body)
                            .foregroundStyle(PawColor.label)
                            .frame(minHeight: PawMetrics.rowOneLine, alignment: .leading)
                            .pawListRow()
                    }
                    if syncing.unpushed > 5 {
                        Text("and \(syncing.unpushed - 5) more")
                            .font(PawFont.footnote)
                            .foregroundStyle(PawColor.labelSecondary)
                            .pawListRow()
                    }
                } header: {
                    PawSectionHeader("Your \(changes) not sent yet")
                        .accessibilityIdentifier("replaced-unpushed")
                } footer: {
                    Text("Kept changes are added to the new store. "
                        + "If the same password changed there too, both versions are kept.")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
            }
            if let problem = syncing.acceptProblem {
                Section {
                    ErrorRow(kind: .error, problem: "Nothing was changed", nextStep: problem)
                        .pawListRow()
                }
            }
            Section {
                Button {
                    confirmingDiscard = false
                    syncing.acceptReplaced(keepChanges: true)
                } label: {
                    Text(syncing.unpushed > 0 ? "Use the new store, keep my \(changes)" : "Use the new store")
                }
                .buttonStyle(.pawPrimary)
                .disabled(syncing.accepting)
                .accessibilityIdentifier("replaced-keep")
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            }
            if syncing.unpushed > 0 {
                Section {
                    if confirmingDiscard {
                        InlineConfirmRow(
                            question: "Discard \(changes)?",
                            detail: "This can't be undone.",
                            confirmTitle: "Discard", cancelTitle: syncing.unpushed == 1 ? "Keep it" : "Keep them",
                            identifier: "replaced-discard",
                            onConfirm: {
                                confirmingDiscard = false
                                syncing.acceptReplaced(keepChanges: false)
                            },
                            onCancel: { confirmingDiscard = false })
                            .pawListRow()
                    } else {
                        Button {
                            confirmingDiscard = true
                        } label: {
                            Text("Use the new store, discard my \(changes)")
                        }
                        .buttonStyle(.pawDestructive)
                        .disabled(syncing.accepting)
                        .accessibilityIdentifier("replaced-discard-button")
                        .listRowInsets(EdgeInsets())
                        .listRowBackground(Color.clear)
                    }
                }
            }
            Section {
                Button("Not now") {
                    syncing.notNow()
                }
                .buttonStyle(.pawSecondary)
                .accessibilityIdentifier("replaced-not-now")
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            } footer: {
                Text("Sync stays paused until you choose.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
        .listStyle(.insetGrouped)
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .navigationTitle("Store replaced")
        .navigationBarTitleDisplayMode(.inline)
    }
}
