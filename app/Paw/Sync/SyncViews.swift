import SwiftUI

extension StoreEdits {
    /// "Send 2 changes", the key sheet's title for a send.
    var sendTitle: String {
        "Send \(changeCount(unpushed.count))"
    }
}

/// A button title after the gold disc: this tap asks for the key.
struct KeyActionLabel: View {
    let title: String

    init(_ title: String) {
        self.title = title
    }

    var body: some View {
        HStack(spacing: PawMetrics.Spacing.s) {
            KeyDisc(size: .marker)
            Text(title)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title). Needs your key.")
    }
}

/// Under the Home title: the sync status as a pill. Tapping it opens the
/// sync sheet; with the disc ("Updates need your key") it goes straight to
/// the key sheet that updates the store. The time it shows moves on by
/// itself; nothing is fetched.
struct HomeSyncPill: View {
    let model: AppModel

    var body: some View {
        if let edits = model.edits, let syncing = model.syncing {
            TimelineView(.periodic(from: .now, by: 30)) { context in
                let state = syncing.pillState(unpushed: edits.unpushed.count)
                let text = state.text(now: context.date)
                Button {
                    syncing.pillTapped()
                } label: {
                    SyncPill(status: state.status, text: text)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(state == .needsKey ? "Sync status: \(text). Needs your key." : "Sync status: \(text)")
                .accessibilityIdentifier("sync-pill")
            }
        }
    }
}

/// After a sync brought something in: what changed, each entry tappable,
/// until dismissed. Both sides changing one entry is said in design.md
/// 2.9's words: both versions were kept, and which is which.
struct SyncSummarySection: View {
    let model: AppModel

    /// Names past this many are counted, not listed.
    private static let shown = 5

    var body: some View {
        if let syncing = model.syncing, let summary = syncing.summary {
            Section {
                HStack(alignment: .firstTextBaseline, spacing: PawMetrics.Spacing.m) {
                    // A tick only when everything simply arrived. Two
                    // versions kept is something to look at, not a success.
                    Image(systemName: summary.needsALook ? PawSymbol.note : PawSymbol.synced)
                        .font(PawFont.body)
                        .foregroundStyle(summary.needsALook ? PawColor.labelSecondary : PawColor.success)
                        .accessibilityHidden(true)
                    Text(summary.title)
                        .font(PawFont.headline)
                        .foregroundStyle(PawColor.label)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityAddTraits(.isHeader)
                        .accessibilityIdentifier("sync-summary-title")
                    Button {
                        syncing.summary = nil
                    } label: {
                        Image(systemName: "xmark")
                            .font(PawFont.footnote.weight(.semibold))
                            .foregroundStyle(PawColor.labelSecondary)
                            .frame(width: PawMetrics.tapMin, height: PawMetrics.tapMin)
                    }
                    .buttonStyle(.borderless)
                    .accessibilityLabel("Dismiss")
                    .accessibilityIdentifier("sync-summary-dismiss")
                }
                .padding(.vertical, PawMetrics.Spacing.xs)
                .pawListRow()
                entries(summary.added, verb: "Added", key: "added")
                entries(summary.changed, verb: "Changed", key: "changed")
                ForEach(summary.removed.prefix(Self.shown), id: \.self) { path in
                    plainRow("Removed \(EntryRef(path: path).name)", detail: EntryRef(path: path).folder,
                             identifier: "sync-removed-\(EntryRef(path: path).name)")
                }
                more(summary.removed.count)
                ForEach(summary.keptBoth, id: \.copy) { pair in
                    keptBoth(pair)
                }
                ForEach(summary.keptLocal, id: \.self) { path in
                    plainRow(
                        "\(EntryRef(path: path).name) was deleted on another device but changed here.",
                        detail: "Your version was kept.", identifier: "sync-kept-local-\(EntryRef(path: path).name)")
                }
                ForEach(summary.droppedDeletes, id: \.self) { path in
                    plainRow(
                        "\(EntryRef(path: path).name) changed on another device, so it wasn't deleted.",
                        detail: "Delete it again if you still want to.",
                        identifier: "sync-dropped-delete-\(EntryRef(path: path).name)")
                }
                ForEach(summary.recipientsChanged, id: \.self) { folder in
                    recipientsRow(folder)
                }
            } footer: {
                if !summary.endedCopies.isEmpty {
                    Text(endedCopiesSentence(summary.endedCopies))
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                        .accessibilityIdentifier("sync-summary-ended-copies")
                }
            }
        }
    }

    @ViewBuilder
    private func entries(_ paths: [String], verb: String, key: String) -> some View {
        ForEach(paths.prefix(Self.shown), id: \.self) { path in
            let entry = EntryRef(path: path)
            EntryRowFor(
                model: model, entry: entry,
                subtitle: entry.folder.isEmpty ? verb : "\(verb) \u{00B7} \(entry.folder)",
                identifier: "sync-\(key)-\(entry.name)")
        }
        more(paths.count)
    }

    @ViewBuilder
    private func more(_ count: Int) -> some View {
        if count > Self.shown {
            Text("and \(count - Self.shown) more")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
                .pawListRow()
        }
    }

    /// "Two versions of github.com were kept", then both, each to open.
    @ViewBuilder
    private func keptBoth(_ pair: KeptBoth) -> some View {
        let entry = EntryRef(path: pair.entry)
        let copy = EntryRef(path: pair.copy)
        Text("\(entry.name) changed here and on another device. Both versions were kept. "
            + "Delete the one you don't need.")
            .font(PawFont.subheadline)
            .foregroundStyle(PawColor.label)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.vertical, PawMetrics.Spacing.s)
            .accessibilityIdentifier("sync-kept-both-\(entry.name)")
            .pawListRow()
        EntryRowFor(
            model: model, entry: entry, subtitle: "From your other device",
            identifier: "sync-kept-both-remote-\(entry.name)")
        EntryRowFor(
            model: model, entry: copy, subtitle: Device.words("From this iPhone"),
            identifier: "sync-kept-both-local-\(entry.name)")
    }

    private func recipientsRow(_ folder: String) -> some View {
        return ErrorRow(
            kind: .warning, problem: whoCanOpenChanged(folder),
            nextStep: "Review the new list before saving there.",
            actionTitle: "Review"
        ) {
            model.show(.sync(.review(folder: folder)))
        }
        .pawListRow()
    }

    private func plainRow(_ title: String, detail: String, identifier: String) -> some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
            Text(title)
                .font(PawFont.body)
                .foregroundStyle(PawColor.label)
            if !detail.isEmpty {
                Text(detail)
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .padding(.vertical, PawMetrics.Spacing.s)
        .frame(maxWidth: .infinity, minHeight: PawMetrics.rowOneLine, alignment: .leading)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(identifier)
        .pawListRow()
    }

    private func endedCopiesSentence(_ paths: [String]) -> String {
        let names = paths.map { EntryRef(path: $0).name }
        let list = names.count == 1 ? names[0] : names.dropLast().joined(separator: ", ") + " and " + (names.last ?? "")
        return names.count == 1
            ? "\(list) changed, so it needs your key again."
            : "\(list) changed, so they need your key again."
    }
}

/// At the top of "Not sent yet": the main button, or why the changes
/// can't go as they are. Never offers to send a held-back change.
struct SyncSendSection: View {
    let model: AppModel

    var body: some View {
        if let edits = model.edits, let syncing = model.syncing, !edits.unpushed.isEmpty {
            Section {
                if !syncing.heldBack.isEmpty {
                    ErrorRow(
                        kind: .warning,
                        problem: "\(changeCount(syncing.heldBack.count)) can't be sent yet",
                        nextStep: "Who can open the folder changed after you saved.",
                        actionTitle: "Review"
                    ) {
                        model.show(.sync(.heldBack))
                    }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("unpushed-held-back")
                    .pawListRow()
                    if syncing.reencryptReady > 0 {
                        Button {
                            syncing.startReencrypt()
                        } label: {
                            KeyActionLabel(reencryptTitle(syncing.reencryptReady))
                        }
                        .buttonStyle(.pawPrimary)
                        .disabled(syncing.running != nil)
                        .accessibilityIdentifier("unpushed-reencrypt")
                        .listRowInsets(EdgeInsets())
                        .listRowBackground(Color.clear)
                    }
                } else if syncing.canSend(unpushed: edits.unpushed.count) {
                    Button {
                        syncing.start(.send)
                    } label: {
                        KeyActionLabel(edits.sendTitle)
                    }
                    .buttonStyle(.pawPrimary)
                    .disabled(syncing.running != nil)
                    .accessibilityIdentifier("unpushed-send")
                    .listRowInsets(EdgeInsets())
                    .listRowBackground(Color.clear)
                } else {
                    Button {
                        syncing.showStatus()
                    } label: {
                        Text("Why these can't be sent yet")
                    }
                    .buttonStyle(.pawSecondary)
                    .accessibilityIdentifier("unpushed-sync-status")
                    .listRowInsets(EdgeInsets())
                    .listRowBackground(Color.clear)
                }
            } footer: {
                Text("To \(syncing.host). Needs your key.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("unpushed-send-footer")
            }
        }
    }
}

/// The sync sheet and the debug element with the simulated host's tip.
/// Apply once, at the root.
private struct SyncModifier: ViewModifier {
    let model: AppModel

    private var syncing: SyncModel? {
        model.syncing
    }

    private var sheet: Binding<SyncSheet?> {
        Binding(
            get: { syncing?.sheet },
            set: { sheet in
                if sheet == nil { syncing?.cancelSheet() }
            })
    }

    func body(content: Content) -> some View {
        content
            .sheet(item: sheet, onDismiss: { syncing?.sheetDismissed() }) { _ in
                if let syncing {
                    SyncSheetHost(model: model, syncing: syncing)
                }
            }
            #if DEBUG && targetEnvironment(simulator)
            .modifier(SimRemoteWatch(model: model))
            #endif
    }
}

extension View {
    /// The sync sheet; see `SyncModifier`.
    func pawSync(_ model: AppModel) -> some View {
        modifier(SyncModifier(model: model))
    }
}

/// One sheet for the status page and the key stages of a send or check.
private struct SyncSheetHost: View {
    let model: AppModel
    let syncing: SyncModel

    private var keyKind: SyncKind? {
        if case .key(let kind)? = syncing.sheet?.mode { return kind }
        return nil
    }

    var body: some View {
        Group {
            if let keyKind {
                KeyTaskSheet(
                    driver: syncing.keyTask,
                    purpose: purpose(keyKind),
                    note: [syncing.keyPathNote, touchNote].compactMap { $0 }.joined(separator: " ").nilIfEmpty,
                    onCancel: { syncing.cancelSheet() })
            } else {
                SyncStatusSheet(model: model, syncing: syncing)
            }
        }
        .keySheetPresentation(keyKind == nil ? nil : syncing.keyTask.stage)
    }

    private func purpose(_ kind: SyncKind) -> String {
        switch kind {
        case .send:
            return model.edits?.sendTitle ?? "Send your changes"
        case .refresh:
            return "Update your store"
        case .reencrypt:
            // "Re-encrypt 2 of 3" once the core has counted them.
            if let progress = syncing.progress, progress.total > 1 {
                return "Re-encrypt \(min(progress.done + 1, progress.total)) of \(progress.total)"
            }
            return reencryptTitle(syncing.reencryptReady)
        }
    }

    /// Said only when the core expects the login to wait for a touch: a
    /// key whose touch is cached and was touched in the last 15 seconds
    /// signs without one.
    private var touchNote: String? {
        guard let policy = model.keyPort.session()?.info().authenticationTouch else { return nil }
        switch syncing.sync.touchExpectation(policy: policy) {
        case .notNeeded, .needed: return nil
        case .mayBeNeeded: return "If it isn't blinking, no touch is needed."
        }
    }
}

/// The status page: when paw last checked, where it syncs to, what went
/// wrong and the next step, and the two things that need the key.
private struct SyncStatusSheet: View {
    let model: AppModel
    let syncing: SyncModel

    private var unpushed: Int {
        model.edits?.unpushed.count ?? 0
    }

    /// The last background run: its result, with its time only when that
    /// differs from "Last checked" above, so "Just now" is said once.
    private func backgroundValue(now: Date) -> String {
        guard let run = syncing.backgroundRun else { return "Not yet" }
        let when = syncAgo(run.at, now: now)
        let result = run.outcome.words
        if let checked = syncing.lastChecked, syncAgo(checked, now: now) == when {
            return result
        }
        return "\(when.capitalizedFirst) \u{00B7} \(result)"
    }

    var body: some View {
        NavigationStack {
            List {
                attention
                Section {
                    LabeledContent {
                        TimelineView(.periodic(from: .now, by: 30)) { context in
                            Text(syncing.lastChecked.map { syncAgo($0, now: context.date).capitalizedFirst }
                                 ?? "Never")
                                .foregroundStyle(PawColor.labelSecondary)
                        }
                    } label: {
                        Text("Last checked")
                            .foregroundStyle(PawColor.label)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("sync-last-checked")
                    if syncing.checksWithSignIn {
                        LabeledContent {
                            TimelineView(.periodic(from: .now, by: 30)) { context in
                                Text(backgroundValue(now: context.date))
                                    .foregroundStyle(PawColor.labelSecondary)
                                    .multilineTextAlignment(.trailing)
                            }
                        } label: {
                            Text("Background check")
                                .foregroundStyle(PawColor.label)
                        }
                        .accessibilityElement(children: .combine)
                        .accessibilityIdentifier("sync-background")
                    }
                    VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                        Text("Store")
                            .foregroundStyle(PawColor.label)
                        Text(syncing.address)
                            .font(PawFont.footnote.monospaced())
                            .foregroundStyle(PawColor.labelSecondary)
                            .textSelection(.enabled)
                    }
                    .padding(.vertical, PawMetrics.Spacing.xs)
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("sync-address")
                    if unpushed > 0 {
                        Button {
                            syncing.go(to: .unpushed)
                        } label: {
                            LabeledContent {
                                Text(changeCount(unpushed))
                                    .foregroundStyle(PawColor.labelSecondary)
                            } label: {
                                Text("Not sent yet")
                                    .foregroundStyle(PawColor.label)
                            }
                        }
                        .accessibilityIdentifier("sync-not-sent")
                    }
                }
                #if DEBUG && targetEnvironment(simulator)
                SimRemoteSection()
                #endif
            }
            .listStyle(.insetGrouped)
            .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
            // Under the list, not in it: the two actions stay on screen at
            // the sheet's first height however much is said above them.
            .safeAreaInset(edge: .bottom, spacing: 0) { actions }
            .navigationTitle("Sync")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { syncing.cancelSheet() }
                        .accessibilityIdentifier("sync-sheet-done")
                }
            }
        }
        .accessibilityIdentifier("sync-sheet")
    }

    /// What went wrong, and what is waiting to be looked at.
    @ViewBuilder private var attention: some View {
        if let failure = syncing.failure, failure.kind != .refusedRemote {
            Section {
                if failure.kind == .cancelled {
                    Text(failure.problem)
                        .font(PawFont.subheadline)
                        .foregroundStyle(PawColor.labelSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.vertical, PawMetrics.Spacing.s)
                        .accessibilityIdentifier("sync-cancelled")
                } else {
                    ErrorRow(
                        kind: failure.kind == .offline ? .warning : .error,
                        problem: failure.problem, nextStep: failure.nextStep,
                        actionTitle: failure.actionTitle,
                        actionNeedsKey: failure.actionNeedsKey && !(failure.during == .refresh && syncing.checksWithSignIn)
                    ) {
                        act(on: failure)
                    }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("sync-failure")
                    if !failure.details.isEmpty {
                        DisclosureGroup("Details") {
                            ForEach(failure.details, id: \.self) { line in
                                Text(line)
                                    .font(PawFont.footnote.monospaced())
                                    .foregroundStyle(PawColor.labelSecondary)
                                    .textSelection(.enabled)
                            }
                        }
                        .font(PawFont.subheadline)
                        .foregroundStyle(PawColor.label)
                        .accessibilityIdentifier("sync-failure-details")
                    }
                }
            }
        }
        if syncing.replaced != nil {
            Section {
                ErrorRow(
                    kind: .warning, problem: "The store on \(syncing.host) was replaced",
                    nextStep: "Sync is paused until you choose what to do.",
                    actionTitle: "Review"
                ) {
                    syncing.go(to: .sync(.replaced))
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("sync-replaced")
            }
        }
        if !syncing.refused.isEmpty {
            Section {
                ErrorRow(
                    kind: .warning, problem: SyncFailure.refusedRemoteProblem,
                    nextStep: SyncFailure.refusedRemoteNextStep(unpushed: unpushed),
                    actionTitle: "Show names"
                ) {
                    syncing.go(to: .sync(.refusedRemote))
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("sync-refused-remote")
            }
        }
        if !syncing.heldBack.isEmpty {
            Section {
                ErrorRow(
                    kind: .warning,
                    problem: "\(changeCount(syncing.heldBack.count)) \(syncing.heldBack.count == 1 ? "wasn't" : "weren't") sent",
                    nextStep: "Who can open the folder changed after you saved.",
                    actionTitle: "Review"
                ) {
                    syncing.go(to: .sync(.heldBack))
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("sync-held-back")
            }
        }
    }

    private func act(on failure: SyncFailure) {
        switch failure.kind {
        case .hostKeyChanged: syncing.openSettings()
        case .refusedRemote: syncing.go(to: .sync(.refusedRemote))
        default: syncing.retryFailed()
        }
    }

    /// Send (or Re-encrypt, while changes are held back) and Check, each
    /// with the gold disc: that says they need the key.
    private var actions: some View {
        let canSend = syncing.canSend(unpushed: unpushed)
        let canReencrypt = !syncing.heldBack.isEmpty && syncing.reencryptReady > 0
        return VStack(spacing: PawMetrics.Spacing.s) {
            if canReencrypt {
                Button {
                    syncing.startReencrypt()
                } label: {
                    KeyActionLabel(reencryptTitle(syncing.reencryptReady))
                }
                .buttonStyle(.pawPrimary)
                .disabled(syncing.running != nil)
                .accessibilityIdentifier("sync-reencrypt")
            }
            if canSend, let edits = model.edits {
                Button {
                    syncing.start(.send)
                } label: {
                    KeyActionLabel(edits.sendTitle)
                }
                .buttonStyle(.pawPrimary)
                .accessibilityIdentifier("sync-send")
            }
            Button {
                syncing.check()
            } label: {
                if syncing.checksWithSignIn {
                    Text(syncing.running == .refresh ? "Checking for changes\u{2026}" : "Check for changes")
                } else {
                    KeyActionLabel("Check for changes")
                }
            }
            .buttonStyle(PawButtonStyle(kind: canSend || canReencrypt ? .secondary : .primary))
            .disabled(syncing.running != nil)
            .accessibilityIdentifier("sync-check")
        }
        .padding(.horizontal, PawMetrics.screenMargin)
        .padding(.top, PawMetrics.Spacing.m)
        .padding(.bottom, PawMetrics.Spacing.s)
        .background(PawColor.bg)
    }
}

extension String {
    /// `nil` for "".
    var nilIfEmpty: String? {
        isEmpty ? nil : self
    }

    /// "Just now" from "just now".
    var capitalizedFirst: String {
        prefix(1).uppercased() + dropFirst()
    }
}
