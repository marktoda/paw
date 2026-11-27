import SwiftUI

/// On Home, under the title: "3 changes not sent", while there are any.
/// Tapping it opens the list. No key is involved, so no gold.
struct UnpushedSection: View {
    let model: AppModel

    var body: some View {
        if let queue = model.storeSession.syncing?.queue, !queue.unpushed.isEmpty {
            Section {
                UnpushedBanner(title: queue.bannerTitle, detail: "Saved on this iPhone.") {
                    model.show(.unpushed)
                }
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            }
        }
    }
}

/// "Not sent yet": what each unpushed change did, newest first, with Undo
/// for the newest, and discarding one or all after an inline confirm.
struct UnpushedScreen: View {
    let model: AppModel

    /// The discard waiting for its confirm.
    private enum Confirming: Equatable {
        case one(String)
        case all
    }

    @State private var confirming: Confirming?

    var body: some View {
        let queue = model.storeSession.syncing?.queue
        let items = queue?.unpushed ?? []
        List {
            if let problem = queue?.problem {
                Section {
                    ErrorRow(kind: .error, problem: "That didn't work", nextStep: problem)
                        .pawListRow()
                }
            }
            if items.isEmpty {
                Section {
                    EmptyState(
                        symbol: PawSymbol.synced, title: "Nothing to send", message: "")
                        .listRowBackground(Color.clear)
                }
            } else {
                SyncSendSection(model: model)
                Section {
                    Button("Undo last change") {
                        confirming = nil
                        queue?.undoLast()
                    }
                    .buttonStyle(.pawSecondary)
                    .accessibilityIdentifier("unpushed-undo-last")
                    .listRowInsets(EdgeInsets())
                    .listRowBackground(Color.clear)
                }
                Section {
                    ForEach(items, id: \.id) { item in
                        if confirming == .one(item.id) {
                            InlineConfirmRow(
                                question: "Discard \u{201C}\(item.title)\u{201D}?",
                                detail: item.discardDetail,
                                confirmTitle: "Discard", cancelTitle: "Keep",
                                identifier: "unpushed-discard-\(EntryRef(path: item.entry).name)",
                                onConfirm: {
                                    confirming = nil
                                    queue?.discard(item.id)
                                },
                                onCancel: { confirming = nil })
                                .pawListRow()
                        } else {
                            QueueRow(item: item) { confirming = .one(item.id) }
                        }
                    }
                } header: {
                    PawSectionHeader(items.count == 1 ? changeCount(1) : "\(changeCount(items.count)), newest first")
                        .accessibilityIdentifier("unpushed-count")
                }
                Section {
                    if confirming == .all {
                        InlineConfirmRow(
                            question: "Discard \(changeCount(items.count))?",
                            detail: "The store goes back to how it was last downloaded.",
                            confirmTitle: "Discard all", cancelTitle: "Keep them", identifier: "unpushed-discard-all",
                            onConfirm: {
                                confirming = nil
                                queue?.discardAll()
                            },
                            onCancel: { confirming = nil })
                            .pawListRow()
                    } else {
                        Button(items.count == 1 ? "Discard this change" : "Discard all \(items.count) changes") {
                            confirming = .all
                        }
                        .buttonStyle(.pawDestructive)
                        .accessibilityIdentifier("unpushed-discard-all")
                        .listRowInsets(EdgeInsets())
                        .listRowBackground(Color.clear)
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .navigationTitle("Not sent yet")
        .navigationBarTitleDisplayMode(.large)
    }
}

private struct QueueRow: View {
    let item: QueueItem
    let onDiscard: () -> Void

    var body: some View {
        let entry = EntryRef(path: item.entry)
        HStack(spacing: PawMetrics.Spacing.m) {
            IconChip(symbol: Self.symbol(item.kind), tint: FolderTint(folder: entry.topFolder))
            VStack(alignment: .leading, spacing: 0) {
                Text(item.title)
                    .font(PawFont.body)
                    .foregroundStyle(PawColor.label)
                    .fixedSize(horizontal: false, vertical: true)
                Text(subtitle(entry))
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("unpushed-\(item.kind)-\(entry.name)")
            Button("Discard", action: onDiscard)
                .buttonStyle(.borderless)
                .font(PawFont.subheadline.weight(.semibold))
                .foregroundStyle(PawColor.label)
                .padding(.horizontal, PawMetrics.Spacing.m)
                .frame(minHeight: PawMetrics.pill)
                .background(PawColor.fill, in: Capsule())
                .frame(minHeight: PawMetrics.tapMin)
                .accessibilityLabel("Discard \(item.title)")
                .accessibilityIdentifier("unpushed-discard-button-\(entry.name)")
        }
        .padding(.vertical, PawMetrics.Spacing.s)
        .frame(minHeight: PawMetrics.rowTwoLine)
        .pawListRow()
    }

    private func subtitle(_ entry: EntryRef) -> String {
        "\(FolderWords.place(entry.folder)) \u{00B7} \(savedWhen(item.when))"
    }

    private static func symbol(_ kind: ChangeKind) -> String {
        switch kind {
        case .added: "plus"
        case .edited: "pencil"
        case .deleted: "trash"
        case .renamed: "arrow.right"
        }
    }
}
