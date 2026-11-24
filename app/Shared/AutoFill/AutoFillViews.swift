import SwiftUI

/// The AutoFill provider's screens, in the app's design language. The
/// extension hosts this; so does the debug harness in the app.
struct AutoFillRootView: View {
    @Bindable var model: AutoFillModel

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("paw")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    if showsCancel {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("Cancel") { model.cancel() }
                                .accessibilityIdentifier("autofill-cancel")
                        }
                    }
                }
                .toolbar(model.screen == .key ? .hidden : .visible, for: .navigationBar)
        }
        .onAppear {
            model.start()
        }
    }

    private var showsCancel: Bool {
        switch model.screen {
        case .list, .faceId, .loading, .busy: true
        default: false
        }
    }

    @ViewBuilder private var content: some View {
        switch model.screen {
        case .loading, .filled:
            PawColor.bg.ignoresSafeArea()
        case .list:
            AutoFillList(model: model)
        case .confirm(let entry, let reason):
            AutoFillConfirm(model: model, entry: entry, reason: reason)
        case .faceId(let entry):
            AutoFillFaceIdView(purpose: model.purpose(entry))
        case .key:
            AutoFillKeySheet(model: model)
        case .noStore:
            EmptyState(
                symbol: PawSymbol.entry, title: "Set up paw first",
                message: "Open paw, then come back here.",
                primary: EmptyState.Action(title: "Close", identifier: "autofill-close") { model.cancel() })
                .background(PawColor.bg.ignoresSafeArea())
        case .busy:
            VStack(spacing: PawMetrics.Spacing.l) {
                ErrorRow(kind: .warning, problem: "The store is busy", nextStep: "paw is using it. Try again in a moment.",
                         isBanner: true)
                Button("Try again") { model.reopen() }
                    .buttonStyle(.pawPrimary)
                    .accessibilityIdentifier("autofill-retry")
                Spacer(minLength: 0)
            }
            .padding(PawMetrics.Spacing.l)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(PawColor.bg.ignoresSafeArea())
        case .problem(let problem, let nextStep, let backToList):
            VStack(spacing: PawMetrics.Spacing.l) {
                ErrorRow(kind: .error, problem: problem, nextStep: nextStep, isBanner: true)
                if backToList {
                    Button("Back to the list") { model.backToList() }
                        .buttonStyle(.pawPrimary)
                        .accessibilityIdentifier("autofill-back")
                }
                Button("Close") { model.cancel() }
                    .buttonStyle(.pawSecondary)
                    .accessibilityIdentifier("autofill-close")
                Spacer(minLength: 0)
            }
            .padding(PawMetrics.Spacing.l)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(PawColor.bg.ignoresSafeArea())
            .accessibilityIdentifier("autofill-problem")
        }
    }
}

/// A row under "Might be for this site", with what search highlighted.
private struct MaybeRow: Identifiable {
    let entry: EntryRef
    var names: [NSRange] = []
    var folders: [NSRange] = []
    var id: String { entry.path }
}

/// The site's entries first, then search over everything.
private struct AutoFillList: View {
    @Bindable var model: AutoFillModel

    var body: some View {
        List {
            if let notice = model.notice {
                Section {
                    ErrorRow(
                        kind: .warning, problem: notice.problem, nextStep: notice.nextStep,
                        actionTitle: notice.keyEntry == nil ? nil : "Use your key",
                        actionNeedsKey: true,
                        action: {
                            if let entry = notice.keyEntry { model.openWithKey(entry) }
                        })
                }
            }
            if model.isSearching {
                searchSection
            } else {
                siteSection
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(PawColor.bg.ignoresSafeArea())
        .searchable(text: $model.query, prompt: Text("Search"))
        .accessibilityIdentifier("autofill-list")
    }

    @ViewBuilder private var siteSection: some View {
        if model.matches.isEmpty && model.maybe.isEmpty {
            Section {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                    Text(model.site.map { "Nothing here matches \($0)." } ?? "paw can't tell which site this is.")
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.label)
                    Text("Search for it by name.")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
                .padding(.vertical, PawMetrics.Spacing.s)
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("autofill-no-match")
            }
        } else {
            if !model.matches.isEmpty {
                Section {
                    ForEach(model.matches, id: \.path) { entry in
                        row(entry)
                    }
                } header: {
                    PawSectionHeader(model.site.map { "For \($0)" } ?? "Suggested")
                        .accessibilityIdentifier("autofill-site-section")
                }
            }
            maybeSection(model.maybe.map { MaybeRow(entry: $0) })
        }
    }

    /// Named like the site, never filled on it: apart, below the site's
    /// own, and each asks first.
    @ViewBuilder private func maybeSection(_ rows: [MaybeRow]) -> some View {
        if !rows.isEmpty {
            Section {
                ForEach(rows) { item in
                    row(item.entry, nameHighlights: item.names, folderHighlights: item.folders)
                }
            } header: {
                PawSectionHeader("Might be for this site")
                    .accessibilityIdentifier("autofill-maybe-section")
            }
        }
    }

    @ViewBuilder private var searchSection: some View {
        if model.hits.isEmpty {
            Section {
                Text("No matches.")
                    .font(PawFont.body)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("autofill-no-hits")
            }
        } else {
            // The site's own entries keep their header; everything else is
            // set apart, and filling one asks first.
            let ours = model.hits.filter { model.isMatch($0.entry) }
            let maybe = model.hits.filter { model.isMaybe($0.entry) }
            let others = model.hits.filter { !model.isMatch($0.entry) && !model.isMaybe($0.entry) }
            if !ours.isEmpty {
                Section {
                    ForEach(ours, id: \.entry.path, content: hitRow)
                } header: {
                    PawSectionHeader(model.site.map { "For \($0)" } ?? "Suggested")
                        .accessibilityIdentifier("autofill-site-section")
                }
            }
            maybeSection(maybe.map {
                MaybeRow(entry: $0.entry, names: $0.nameRanges.map(Self.range), folders: $0.folderRanges.map(Self.range))
            })
            if !others.isEmpty {
                Section {
                    ForEach(others, id: \.entry.path, content: hitRow)
                } header: {
                    PawSectionHeader("Other passwords")
                        .accessibilityIdentifier("autofill-other-section")
                }
            }
        }
    }

    private func hitRow(_ hit: SearchHit) -> some View {
        row(hit.entry,
            nameHighlights: hit.nameRanges.map(Self.range),
            folderHighlights: hit.folderRanges.map(Self.range))
    }

    private func row(_ entry: EntryRef, nameHighlights: [NSRange] = [], folderHighlights: [NSRange] = []) -> some View {
        EntryRow(
            name: entry.name,
            subtitle: entry.folder,
            symbol: EntryGlyph.symbol(for: entry.name),
            tint: FolderTint(folder: entry.folder.split(separator: "/").first.map(String.init)),
            access: model.live.contains(entry.path) ? .faceID : .needsKey,
            nameHighlights: nameHighlights,
            subtitleHighlights: folderHighlights,
            identifier: "autofill-entry-\(entry.path)"
        ) {
            model.pick(entry)
        }
        .pawListRow()
    }

    private static func range(_ range: HighlightRange) -> NSRange {
        NSRange(location: Int(range.start), length: Int(range.len))
    }
}

/// An entry that is not this site's, or is this site's by name only and
/// has not filled on this domain, was picked: say so before Face ID or the
/// key, inline. Not filling is the primary action.
private struct AutoFillConfirm: View {
    let model: AutoFillModel
    let entry: EntryRef
    let reason: AutoFillConfirmReason

    private var headline: String {
        switch reason {
        case .otherSite:
            model.site.map { "Fill \(entry.name)'s password on \($0)?" } ?? "Fill \(entry.name)'s password?"
        case .firstTime(let site), .lookalike(let site, _):
            "Use \(entry.name) on \(site)?"
        }
    }

    private var line: String {
        switch reason {
        case .otherSite:
            model.site == nil ? "paw can't tell which site this is." : "This password isn't saved for this site."
        case .firstTime:
            "First time on this site. Check the address is right."
        case .lookalike(let site, let known):
            "You've used this on \(ListFormatter.localizedString(byJoining: known)). This is \(site)."
        }
    }

    private var isLookalike: Bool {
        if case .lookalike = reason { return true }
        return false
    }

    private var isOtherSite: Bool {
        reason == .otherSite
    }

    /// The full address, for a name match: the registrable domain is in
    /// the headline, and here the host as iOS gave it when it differs.
    private var address: String? {
        switch reason {
        case .otherSite: nil
        case .firstTime(let site), .lookalike(let site, _): model.site.flatMap { $0 == site ? nil : $0 }
        }
    }

    private var isFirstTime: Bool {
        if case .firstTime = reason { return true }
        return false
    }

    var body: some View {
        VStack(spacing: PawMetrics.Spacing.l) {
            if isFirstTime {
                // The common case for a store named without domains: a
                // question, not a warning (warning colour is for what must
                // be read before going on).
                VStack(spacing: PawMetrics.Spacing.xs) {
                    Text(headline)
                        .font(PawFont.columnTitle)
                        .foregroundStyle(PawColor.label)
                    Text(line)
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity)
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("autofill-confirm-question")
            } else {
                ErrorRow(kind: isLookalike ? .error : .warning, problem: headline, nextStep: line, isBanner: true)
            }
            if let address {
                Text(address)
                    .font(PawFont.headline.monospaced())
                    .foregroundStyle(PawColor.label)
                    .multilineTextAlignment(.center)
                    .accessibilityLabel("Address \(address)")
                    .accessibilityIdentifier("autofill-confirm-address")
            }
            Button("Cancel") { model.backToList() }
                .buttonStyle(.pawPrimary)
                .accessibilityIdentifier("autofill-confirm-cancel")
            Button(isOtherSite ? "Fill anyway" : "Use it here") { model.fillAnyway() }
                .buttonStyle(.pawSecondary)
                .accessibilityIdentifier(isOtherSite ? "autofill-fill-anyway" : "autofill-use-here")
            Spacer(minLength: 0)
        }
        .padding(PawMetrics.Spacing.l)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(PawColor.bg.ignoresSafeArea())
    }
}

/// Behind the system's Face ID prompt: what is being filled, and where.
private struct AutoFillFaceIdView: View {
    let purpose: String

    var body: some View {
        VStack(spacing: PawMetrics.Spacing.l) {
            Spacer(minLength: 0)
            Image(systemName: PawSymbol.faceID)
                .font(.system(size: PawMetrics.discHero * 0.6))
                .foregroundStyle(PawColor.labelSecondary)
                .accessibilityHidden(true)
            Text(purpose)
                .font(PawFont.title)
                .foregroundStyle(PawColor.label)
                .multilineTextAlignment(.center)
            Spacer(minLength: 0)
        }
        .padding(PawMetrics.Spacing.xl)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(PawColor.bg.ignoresSafeArea())
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("autofill-face-id")
    }
}

/// The app's key sheet, inline: the provider is already a sheet.
private struct AutoFillKeySheet: View {
    @Bindable var model: AutoFillModel

    var body: some View {
        KeySheetContent(
            purpose: model.keyEntry.map(model.purpose) ?? "",
            stage: model.keyStage,
            pin: $model.pinEntry,
            note: model.keepsCopies
                ? "Then it fills with Face ID for \(AutoFillModel.faceIdWindowSecs / 3600) hours." : nil,
            onContinue: { model.continueWithPin() },
            onCancel: { model.cancelKey() },
            onRetry: { model.retry() },
            simulatorAction: simulatorAction,
            moreSimulatorActions: moreSimulatorActions)
            .background(PawColor.surface.ignoresSafeArea())
    }

    private var simulatorAction: SimulatorAction? {
        #if targetEnvironment(simulator)
        switch model.keyStage {
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

    private var moreSimulatorActions: [SimulatorAction] {
        #if targetEnvironment(simulator)
        guard model.keyStage == .touch else { return [] }
        return [SimulatorAction(title: "time out the touch", identifier: "simulated-touch-timeout") {
            KeyConnection.simulateTouchTimeout()
        }]
        #else
        return []
        #endif
    }
}
