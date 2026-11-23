import SwiftUI

extension StoreEdits {
    /// "Send 2 changes", the key sheet's title for a send.
    var sendTitle: String {
        unpushed.count == 1 ? "Send 1 change" : "Send \(unpushed.count) changes"
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
        if let edits = model.edits, let syncing = edits.syncing {
            TimelineView(.periodic(from: .now, by: 30)) { context in
                let state = syncing.pillState(unpushed: edits.unpushed.count)
                let text = state.text(now: context.date)
                Button {
                    syncing.pillTapped(app: model)
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
        if let syncing = model.edits?.syncing, let summary = syncing.summary {
            Section {
                HStack(alignment: .firstTextBaseline, spacing: PawMetrics.Spacing.m) {
                    Image(systemName: PawSymbol.synced)
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.success)
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
                        "\(EntryRef(path: path).name) was changed on another device, so it wasn't deleted.",
                        detail: "Delete it again if you still want it gone.",
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
        Text("\(entry.name) was changed here and on another device. Both versions were kept: "
            + "\u{201C}\(entry.name)\u{201D} is the other device's, \u{201C}\(copy.name)\u{201D} is this iPhone's. "
            + "Open both to compare, then delete the one you don't need.")
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
            model: model, entry: copy, subtitle: "From this iPhone",
            identifier: "sync-kept-both-local-\(entry.name)")
    }

    private func recipientsRow(_ folder: String) -> some View {
        let place = folder.isEmpty ? "your store" : folder
        return ErrorRow(
            kind: .warning, problem: "Who can open \(place) changed.",
            nextStep: "paw won't save there until you've looked at the new list.",
            actionTitle: "Review who can open \(place)"
        ) {
            model.path.append(.sync(.review(folder: folder)))
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
        return "The Face ID copy of \(list) ended because it changed. Open it with your key to see the new version."
    }
}

/// At the top of "Not sent yet": the main button, or why the changes
/// can't go as they are. Never offers to send a held-back change.
struct SyncSendSection: View {
    let model: AppModel

    var body: some View {
        if let edits = model.edits, let syncing = edits.syncing, !edits.unpushed.isEmpty {
            Section {
                if !syncing.heldBack.isEmpty {
                    ErrorRow(
                        kind: .warning,
                        problem: syncing.heldBack.count == 1
                            ? "1 change can't be sent as it is."
                            : "\(syncing.heldBack.count) changes can't be sent as they are.",
                        nextStep: "Who can open their folder changed after they were saved.",
                        actionTitle: "Review"
                    ) {
                        model.path.append(.sync(.heldBack))
                    }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("unpushed-held-back")
                    .pawListRow()
                } else if syncing.canSend(unpushed: edits.unpushed.count) {
                    Button {
                        syncing.start(.send, app: model)
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
                Text("To \(syncing.host). Sending needs your key: its PIN if paw doesn't have it, and a touch.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("unpushed-send-footer")
            }
        }
    }
}

/// The sync sheet, the debug element with the simulated host's tip, and
/// recomputing the pill from local state on coming back to the
/// foreground. Apply once, at the root.
private struct SyncModifier: ViewModifier {
    let model: AppModel
    @Environment(\.scenePhase) private var scenePhase

    private var syncing: SyncModel? {
        model.edits?.syncing
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
            .sheet(item: sheet, onDismiss: { syncing?.sheetDismissed(app: model) }) { _ in
                if let syncing {
                    SyncSheetHost(model: model, syncing: syncing)
                }
            }
            .onChange(of: scenePhase) {
                // Never the network or the key: only what is on this iPhone.
                if scenePhase == .active { syncing?.recomputeLocal() }
            }
            .onChange(of: model.edits?.unpushed) {
                syncing?.recomputeLocal()
            }
            #if targetEnvironment(simulator)
            .background(alignment: .topLeading) {
                if let syncing {
                    Text(syncing.simSummary)
                        .font(.caption2)
                        .frame(width: 1, height: 1)
                        .opacity(0.02)
                        .accessibilityIdentifier("sim-remote-summary")
                }
            }
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
    @Bindable var syncing: SyncModel
    @State private var detent = PresentationDetent.height(PawMetrics.keySheetHeight)

    private var keyStage: (kind: SyncKind, stage: KeySheetStage)? {
        if case .key(let kind, _, let stage)? = syncing.sheet?.mode {
            return (kind, stage)
        }
        return nil
    }

    private var needsKeyboard: Bool {
        switch keyStage?.stage {
        case .pin?, .wrongPin?, .pinAgain?: true
        default: false
        }
    }

    var body: some View {
        Group {
            if let keyStage {
                KeySheetContent(
                    purpose: purpose(keyStage.kind),
                    stage: keyStage.stage,
                    pin: $syncing.pinEntry,
                    note: [syncing.keyPathNote, touchNote].compactMap { $0 }.joined(separator: " ").nilIfEmpty,
                    onContinue: { syncing.continueWithPin() },
                    onCancel: { syncing.cancelSheet() },
                    onRetry: { syncing.retry() },
                    simulatorAction: simulatorAction(keyStage.stage))
            } else {
                SyncStatusSheet(model: model, syncing: syncing)
            }
        }
        .presentationDetents([.height(PawMetrics.keySheetHeight), .large], selection: $detent)
        .presentationDragIndicator(.visible)
        .onChange(of: needsKeyboard, initial: true) {
            detent = needsKeyboard ? .large : .height(PawMetrics.keySheetHeight)
        }
        .privacyCover()
    }

    private func purpose(_ kind: SyncKind) -> String {
        switch kind {
        case .send: model.edits?.sendTitle ?? "Send your changes"
        case .refresh: "Update your store"
        }
    }

    /// Said only when the core expects the login to wait for a touch: a
    /// key whose touch is cached and was touched in the last 15 seconds
    /// signs without one.
    private var touchNote: String? {
        guard let policy = model.keyPort.session()?.info().authenticationTouch else { return nil }
        switch syncing.sync.touchExpectation(policy: policy) {
        case .notNeeded: return nil
        case .needed: return "One touch of your key signs paw in to \(syncing.host)."
        case .mayBeNeeded: return "Touch your key if it blinks: that signs paw in to \(syncing.host)."
        }
    }

    private func simulatorAction(_ stage: KeySheetStage) -> SimulatorAction? {
        #if targetEnvironment(simulator)
        switch stage {
        case .touch:
            return SimulatorAction(title: "touch the key", identifier: "simulated-touch") {
                KeyConnection.simulateTouch()
            }
        case .plugIn:
            return SimulatorAction(title: "plug in the key", identifier: "simulated-plug-in") {
                model.simulatedPlugIn()
            }
        default:
            return nil
        }
        #else
        return nil
        #endif
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

    var body: some View {
        NavigationStack {
            List {
                attention
                Section {
                    LabeledContent {
                        TimelineView(.periodic(from: .now, by: 30)) { context in
                            Text(syncing.lastChecked.map { syncAgo($0, now: context.date).capitalizedFirst }
                                 ?? "Not yet from this iPhone")
                                .foregroundStyle(PawColor.labelSecondary)
                        }
                    } label: {
                        Text("Last checked")
                            .foregroundStyle(PawColor.label)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("sync-last-checked")
                    VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                        Text("Git remote")
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
                                Text(unpushed == 1 ? "1 change" : "\(unpushed) changes")
                                    .foregroundStyle(PawColor.labelSecondary)
                            } label: {
                                Text("Not sent yet")
                                    .foregroundStyle(PawColor.label)
                            }
                        }
                        .accessibilityIdentifier("sync-not-sent")
                    }
                }
                actions
                #if targetEnvironment(simulator)
                SyncSimulatorSection(syncing: syncing)
                #endif
            }
            .listStyle(.insetGrouped)
            .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
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
                    Text("\(failure.problem) \(failure.nextStep)")
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
                    kind: .warning, problem: "The store on your git remote was replaced.",
                    nextStep: "Its history no longer matches this iPhone's. Nothing here has been changed, "
                        + "and nothing syncs until you choose what to do.",
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
                    kind: .warning, problem: "Your git remote contains something paw won't check out.",
                    nextStep: "\(SyncFailure.waiting(unpushed)) It has to be fixed from another computer.",
                    actionTitle: "See what"
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
                    problem: syncing.heldBack.count == 1
                        ? "1 change wasn't sent." : "\(syncing.heldBack.count) changes weren't sent.",
                    nextStep: "Who can open their folder changed after they were saved, so paw won't send them as they are.",
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
        default: syncing.retryFailed(app: model)
        }
    }

    @ViewBuilder private var actions: some View {
        let canSend = syncing.canSend(unpushed: unpushed)
        Section {
            if canSend, let edits = model.edits {
                Button {
                    syncing.start(.send, app: model)
                } label: {
                    KeyActionLabel(edits.sendTitle)
                }
                .buttonStyle(.pawPrimary)
                .accessibilityIdentifier("sync-send")
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            }
            Button {
                syncing.check(app: model)
            } label: {
                if syncing.checksWithSignIn {
                    Text(syncing.running == .refresh ? "Checking for changes\u{2026}" : "Check for changes")
                } else {
                    KeyActionLabel("Check for changes")
                }
            }
            .buttonStyle(PawButtonStyle(kind: canSend ? .secondary : .primary))
            .disabled(syncing.running != nil)
            .accessibilityIdentifier("sync-check")
            .listRowInsets(EdgeInsets(top: PawMetrics.Spacing.s, leading: 0, bottom: 0, trailing: 0))
            .listRowBackground(Color.clear)
        } footer: {
            Text(syncing.checksWithSignIn
                 ? (canSend ? "Sending needs your key. Checking uses your GitHub sign-in." : "Checking uses your GitHub sign-in.")
                 : canSend
                 ? "Both need your key: its PIN if paw doesn't have it, and a touch. Checking brings in changes made elsewhere without sending yours."
                 : "Checking needs your key: its PIN if paw doesn't have it, and a touch.")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
                .padding(.top, PawMetrics.Spacing.s)
        }
    }
}

#if targetEnvironment(simulator)
/// Debug controls for the simulated git host. Simulator only, and drawn so
/// a screenshot never shows them as part of the design.
private struct SyncSimulatorSection: View {
    let syncing: SyncModel

    var body: some View {
        Section {
            ForEach(SimRemoteChange.allCases) { change in
                Button(change.title) { syncing.applySimChange(change) }
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("sim-remote-\(change.rawValue)")
            }
            Button(SimRemoteHandle.shared.refusingPushes ? "Accept pushes again" : "Refuse pushes") {
                syncing.setSimRefusesPushes(!SimRemoteHandle.shared.refusingPushes)
            }
            .foregroundStyle(PawColor.label)
            .accessibilityIdentifier("sim-remote-refuse-pushes")
            Button("Drop the next connection") { syncing.simDropNextConnection() }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-remote-drop")
            Button("Present another host key") { syncing.setSimChangedHostKey(true) }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-remote-host-key")
            Text(syncing.simStatus.isEmpty ? syncing.simSummary : "\(syncing.simStatus) \(syncing.simSummary)")
                .font(PawFont.footnote.monospaced())
                .foregroundStyle(PawColor.labelSecondary)
                .accessibilityIdentifier("sim-remote-status")
        } header: {
            Label("Simulator", systemImage: "hammer")
                .font(PawFont.footnote.weight(.semibold))
                .foregroundStyle(PawColor.labelSecondary)
        }
    }
}
#endif

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
