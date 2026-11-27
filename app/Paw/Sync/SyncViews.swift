import SwiftUI

/// "and 3 more", under a list that names the first `namedAtMost`.
struct AndMoreRow: View {
    let count: Int

    var body: some View {
        if let more = andMore(count) {
            Text(more)
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
                .pawListRow()
        }
    }
}

/// "Re-encrypt 3 changes": shown while `SyncModel.canReencrypt`.
struct ReencryptButton: View {
    let syncing: SyncModel
    let identifier: String

    var body: some View {
        Button {
            syncing.startReencrypt()
        } label: {
            KeyButtonLabel(title: reencryptTitle(syncing.reencryptReady))
        }
        .buttonStyle(.pawPrimary)
        .disabled(syncing.running != nil)
        .accessibilityIdentifier(identifier)
    }
}

/// "Send 2 changes": shown while `SyncModel.canSend`.
struct SendButton: View {
    let syncing: SyncModel
    let identifier: String

    var body: some View {
        Button {
            syncing.start(.send)
        } label: {
            KeyButtonLabel(title: syncing.queue.sendTitle)
        }
        .buttonStyle(.pawPrimary)
        .disabled(syncing.running != nil)
        .accessibilityIdentifier(identifier)
    }
}

/// "Check for changes": with the gold disc when the check asks for the key,
/// and off while any sync is under way. The caller gives the style.
struct CheckButton: View {
    let syncing: SyncModel
    let title: String
    let identifier: String

    var body: some View {
        Button {
            syncing.check()
        } label: {
            if syncing.checkNeedsKey {
                KeyButtonLabel(title: title)
            } else {
                Text(syncing.running == .refresh ? SyncWords.checking : title)
            }
        }
        .disabled(syncing.running != nil)
        .accessibilityIdentifier(identifier)
    }
}

/// Under the Home title: the sync status as a pill. The time it shows moves
/// on by itself; nothing is fetched.
struct HomeSyncPill: View {
    let model: AppModel

    var body: some View {
        if let syncing = model.storeSession.syncing {
            TimelineView(.periodic(from: .now, by: 30)) { context in
                let state = syncing.pillState
                let text = state.text(now: context.date)
                Button {
                    syncing.pillTapped()
                } label: {
                    SyncPill(status: state.status, text: text)
                }
                .buttonStyle(.plain)
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

    var body: some View {
        if let syncing = model.storeSession.syncing, let summary = syncing.summary {
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
                ForEach(summary.removed.prefix(namedAtMost), id: \.self) { path in
                    plainRow("Removed \(EntryRef(path: path).name)", detail: EntryRef(path: path).folder,
                             identifier: "sync-removed-\(EntryRef(path: path).name)")
                }
                AndMoreRow(count: summary.removed.count)
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
        ForEach(paths.prefix(namedAtMost), id: \.self) { path in
            let entry = EntryRef(path: path)
            EntryRowFor(
                model: model, entry: entry,
                subtitle: entry.folder.isEmpty ? verb : "\(verb) \u{00B7} \(entry.folder)",
                identifier: "sync-\(key)-\(entry.name)")
        }
        AndMoreRow(count: paths.count)
    }

    /// Both sides changed one entry: that, then both versions, each to open.
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
            nextStep: reviewBeforeSaving(here: false),
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
        if let syncing = model.storeSession.syncing, !syncing.queue.unpushed.isEmpty {
            Section {
                if !syncing.heldBack.isEmpty {
                    ErrorRow(
                        kind: .warning,
                        problem: "\(changeCount(syncing.heldBack.count)) can't be sent yet",
                        nextStep: SyncWords.heldBackReason,
                        actionTitle: "Review"
                    ) {
                        model.show(.sync(.heldBack))
                    }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("unpushed-held-back")
                    .pawListRow()
                    if syncing.canReencrypt {
                        ReencryptButton(syncing: syncing, identifier: "unpushed-reencrypt")
                            .listRowInsets(EdgeInsets())
                            .listRowBackground(Color.clear)
                    }
                } else if syncing.canSend {
                    SendButton(syncing: syncing, identifier: "unpushed-send")
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
                // Under Send only: it says where Send goes.
                if syncing.heldBack.isEmpty && syncing.canSend {
                    Text("To \(syncing.host). Needs your key.")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                        .accessibilityIdentifier("unpushed-send-footer")
                }
            }
        }
    }
}

/// The sync sheet and the debug element with the simulated host's tip.
/// Apply once, at the root.
private struct SyncModifier: ViewModifier {
    let model: AppModel

    private var syncing: SyncModel? {
        model.storeSession.syncing
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
            .pawSheet(item: sheet, onDismiss: { syncing?.sheetDismissed() }) { _ in
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
    func pawSync(_ model: AppModel) -> some View {
        modifier(SyncModifier(model: model))
    }
}

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
                    note: [syncing.keyPathNote, keyKind == .reencrypt ? nil : touchNote]
                        .compactMap { $0 }.joined(separator: " ").nilIfEmpty,
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
            return syncing.queue.sendTitle
        case .refresh:
            return "Update your store"
        case .reencrypt:
            if let progress = syncing.progress, progress.total > 1 {
                return "Re-encrypt \(min(progress.done + 1, progress.total)) of \(progress.total)"
            }
            return reencryptTitle(syncing.reencryptReady)
        }
    }

    /// Said only when the core expects the login to wait for a touch: a
    /// key whose touch is cached and was touched in the last 15 seconds
    /// signs without one. A re-encrypt logs in nowhere.
    private var touchNote: String? {
        guard let session = model.storeSession.keyPort.session() else { return nil }
        switch session.touchExpectation(slot: .authentication) {
        case .notNeeded, .needed: return nil
        case .mayBeNeeded: return "If it isn't blinking, no touch is needed."
        }
    }
}

private struct SyncStatusSheet: View {
    let model: AppModel
    let syncing: SyncModel

    /// The last background run: its result, with its time only when that
    /// differs from "Last checked" above, so "Just now" is said once.
    private func backgroundValue(now: Date) -> String {
        guard let run = syncing.backgroundRun else { return "Not yet" }
        let when = syncAgo(run.at, now: now)
        let result = run.outcome.words
        if let checked = syncing.lastChecked, syncAgo(checked, now: now) == when {
            return result
        }
        return "\(capitalised(when)) \u{00B7} \(result)"
    }

    var body: some View {
        NavigationStack {
            List {
                attention
                Section {
                    LabeledContent {
                        TimelineView(.periodic(from: .now, by: 30)) { context in
                            Text(syncing.lastChecked.map { capitalised(syncAgo($0, now: context.date)) }
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
                    if syncing.unpushed > 0 {
                        Button {
                            syncing.go(to: .unpushed)
                        } label: {
                            LabeledContent {
                                Text(changeCount(syncing.unpushed))
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
                        actionTitle: failure.step?.title,
                        actionNeedsKey: syncing.actionNeedsKey(after: failure)
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
                    kind: .warning, problem: SyncWords.replaced(host: syncing.host),
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
                    nextStep: SyncFailure.refusedRemoteNextStep(unpushed: syncing.unpushed),
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
                    nextStep: SyncWords.heldBackReason,
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
        switch failure.step {
        case .tryAgain?: syncing.retryFailed()
        case .openSettings?: syncing.openSettings()
        case nil: break
        }
    }

    /// Re-encrypt and Send when they are offered, then Check; the gold disc
    /// marks each that asks for the key.
    private var actions: some View {
        VStack(spacing: PawMetrics.Spacing.s) {
            if syncing.canReencrypt {
                ReencryptButton(syncing: syncing, identifier: "sync-reencrypt")
            }
            if syncing.canSend {
                SendButton(syncing: syncing, identifier: "sync-send")
            }
            CheckButton(syncing: syncing, title: "Check for changes", identifier: "sync-check")
                .buttonStyle(PawButtonStyle(kind: syncing.canSend || syncing.canReencrypt ? .secondary : .primary))
        }
        .padding(.horizontal, PawMetrics.screenMargin)
        .padding(.top, PawMetrics.Spacing.m)
        .padding(.bottom, PawMetrics.Spacing.s)
        .background(PawColor.bg)
    }
}

extension String {
    var nilIfEmpty: String? {
        isEmpty ? nil : self
    }
}
