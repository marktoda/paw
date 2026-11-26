import SwiftUI

/// On Home, under the title: "3 changes not sent", while there are any.
/// Tapping it opens the list. No key is involved, so no gold.
struct UnpushedSection: View {
    let model: AppModel

    var body: some View {
        if let queue = model.storeSession.syncing?.queue, !queue.unpushed.isEmpty {
            Section {
                UnpushedBanner(
                    reason: .waiting, title: queue.bannerTitle,
                    detail: "Saved on this iPhone."
                ) {
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

    /// The change whose discard is waiting for its confirm; "all" for all.
    @State private var confirming: String?

    var body: some View {
        let edits = model.storeSession.edits
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
                        edits?.undoLastChange()
                    }
                    .buttonStyle(.pawSecondary)
                    .accessibilityIdentifier("unpushed-undo-last")
                    .listRowInsets(EdgeInsets())
                    .listRowBackground(Color.clear)
                }
                Section {
                    ForEach(items, id: \.id) { item in
                        if confirming == item.id {
                            InlineConfirmRow(
                                question: "Discard \u{201C}\(Self.title(item))\u{201D}?",
                                detail: Self.discardDetail(item),
                                confirmTitle: "Discard", cancelTitle: "Keep",
                                identifier: "unpushed-discard-\(Self.name(item.entry))",
                                onConfirm: {
                                    confirming = nil
                                    edits?.discardChange(item.id)
                                },
                                onCancel: { confirming = nil })
                                .pawListRow()
                        } else {
                            QueueRow(item: item) { confirming = item.id }
                        }
                    }
                } header: {
                    PawSectionHeader(items.count == 1 ? changeCount(1) : "\(changeCount(items.count)), newest first")
                        .accessibilityIdentifier("unpushed-count")
                }
                Section {
                    if confirming == "all" {
                        InlineConfirmRow(
                            question: "Discard \(changeCount(items.count))?",
                            detail: "The store goes back to how it was last downloaded.",
                            confirmTitle: "Discard all", cancelTitle: "Keep them", identifier: "unpushed-discard-all",
                            onConfirm: {
                                confirming = nil
                                edits?.discardAllChanges()
                            },
                            onCancel: { confirming = nil })
                            .pawListRow()
                    } else {
                        Button(items.count == 1 ? "Discard this change" : "Discard all \(items.count) changes") {
                            confirming = "all"
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

    static func name(_ path: String) -> String {
        EntryRef(path: path).name
    }

    /// "Added github.com", "Renamed vpn to vpn-old".
    static func title(_ item: QueueItem) -> String {
        let name = name(item.entry)
        switch item.kind {
        case .added: return "Added \(name)"
        case .edited: return "Edited \(name)"
        case .deleted: return "Deleted \(name)"
        case .renamed:
            guard let previous = item.previous else { return "Renamed \(name)" }
            let before = EntryRef(path: previous)
            let after = EntryRef(path: item.entry)
            return before.folder == after.folder
                ? "Renamed \(before.name) to \(after.name)"
                : "Moved \(after.name) to \(after.folder.isEmpty ? "the top" : after.folder)"
        }
    }

    static func discardDetail(_ item: QueueItem) -> String {
        let name = name(item.entry)
        switch item.kind {
        case .added: return "\(name) is removed from this \(Device.name)."
        case .edited: return "\(name) goes back to how it was."
        case .deleted: return "\(name) comes back."
        case .renamed: return "\(name) gets its old name back."
        }
    }

    /// "14:02" today, "yesterday", "Mon" this week, else "3 Nov".
    static func when(_ seconds: Int64, now: Date = Date()) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(seconds))
        let calendar = Calendar.current
        if calendar.isDateInToday(date) {
            return date.formatted(date: .omitted, time: .shortened)
        }
        if calendar.isDateInYesterday(date) {
            return "yesterday"
        }
        if let days = calendar.dateComponents([.day], from: date, to: now).day, days < 7 {
            return date.formatted(.dateTime.weekday(.abbreviated))
        }
        return date.formatted(.dateTime.day().month(.abbreviated))
    }
}

/// One unpushed change: what it did, where, when, and Discard.
private struct QueueRow: View {
    let item: QueueItem
    let onDiscard: () -> Void

    var body: some View {
        let entry = EntryRef(path: item.entry)
        HStack(spacing: PawMetrics.Spacing.m) {
            IconChip(symbol: Self.symbol(item.kind), tint: FolderTint(folder: entry.topFolder))
            VStack(alignment: .leading, spacing: 0) {
                Text(UnpushedScreen.title(item))
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
                .accessibilityLabel("Discard \(UnpushedScreen.title(item))")
                .accessibilityIdentifier("unpushed-discard-button-\(entry.name)")
        }
        .padding(.vertical, PawMetrics.Spacing.s)
        .frame(minHeight: PawMetrics.rowTwoLine)
        .pawListRow()
    }

    private func subtitle(_ entry: EntryRef) -> String {
        let place = entry.folder.isEmpty ? "Not in a folder" : entry.folder
        return "\(place) \u{00B7} \(UnpushedScreen.when(item.when))"
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
