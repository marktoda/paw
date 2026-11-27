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
                ErrorRow(
                    kind: .warning, problem: StoreBusyWords.problem, nextStep: StoreBusyWords.nextStep, isBanner: true)
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
            NoticeSection(notice: model.notice) { model.openWithKey($0) }
            if model.isSearching {
                searchSection
            } else {
                siteSection
            }
        }
        .listStyle(.insetGrouped)
        .padReadableWidth()
        .scrollContentBackground(.hidden)
        .background(PawColor.bg.ignoresSafeArea())
        .searchable(text: $model.query, prompt: Text("Search"))
        .accessibilityIdentifier("autofill-list")
    }

    @ViewBuilder private var siteSection: some View {
        if model.listedMatches.isEmpty && model.listedMaybe.isEmpty {
            Section {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                    Text(emptyLine)
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.label)
                    if let site = model.site {
                        // As iOS gave it, `xn--` and all: a lookalike shows here.
                        Text(site)
                            .font(PawFont.footnote.monospaced())
                            .foregroundStyle(PawColor.labelSecondary)
                    }
                }
                .padding(.vertical, PawMetrics.Spacing.s)
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("autofill-no-match")
            }
        } else {
            if !model.listedMatches.isEmpty {
                Section {
                    ForEach(model.listedMatches, id: \.path) { entry in
                        row(entry)
                    }
                } header: {
                    siteHeader
                }
            }
            maybeSection(model.listedMaybe.map { MaybeRow(entry: $0) })
        }
    }

    private var siteHeader: some View {
        PawSectionHeader(model.site.map { "For \($0)" } ?? "Suggested")
            .accessibilityIdentifier("autofill-site-section")
    }

    private var emptyLine: String {
        model.site == nil ? "paw can't tell which site this is." : model.kind.noneForSite
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
                    siteHeader
                }
            }
            maybeSection(maybe.map {
                MaybeRow(entry: $0.entry, names: $0.nameRanges.map(\.nsRange), folders: $0.folderRanges.map(\.nsRange))
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
            nameHighlights: hit.nameRanges.map(\.nsRange),
            folderHighlights: hit.folderRanges.map(\.nsRange))
    }

    private func row(_ entry: EntryRef, nameHighlights: [NSRange] = [], folderHighlights: [NSRange] = []) -> some View {
        EntryRow(
            name: entry.name,
            subtitle: entry.folder,
            symbol: EntryGlyph.symbol(for: entry.name),
            tint: FolderTint(folder: entry.topFolder),
            access: model.opensWithKey(entry) ? .needsKey : .faceID,
            nameHighlights: nameHighlights,
            subtitleHighlights: folderHighlights,
            identifier: "autofill-entry-\(entry.path)"
        ) {
            model.pick(entry)
        }
        .pawListRow()
    }
}

/// An entry that is not this site's, or is this site's by name only and
/// has not filled on this domain, was picked: say so before Face ID or the
/// key, inline. Or it was opened and its `url:` line names another site:
/// say so before it fills.
private struct AutoFillConfirm: View {
    let model: AutoFillModel
    let entry: EntryRef
    let reason: AutoFillConfirmReason

    private enum Tone {
        /// A question: the first fill of a name match.
        case neutral
        /// Not this site's password, found by search.
        case warning
        /// The entry says it belongs to another site.
        case danger
    }

    private var tone: Tone {
        switch reason {
        case .firstTime: .neutral
        case .otherSite: .warning
        case .lookalike, .urlDiffers: .danger
        }
    }

    private var headline: String {
        switch reason {
        case .otherSite:
            // The address below names the site.
            model.site == nil
                ? "Fill \(entry.name)'s \(model.kind.thing)?" : "Fill \(entry.name)'s \(model.kind.thing) here?"
        case .firstTime(let site), .lookalike(let site, _):
            "Use \(entry.name) on \(site)?"
        case .urlDiffers(let url, let site):
            "Saved for \(url), not \(site)"
        }
    }

    private var line: String {
        switch reason {
        case .otherSite:
            model.site == nil ? "paw can't tell which site this is." : "This \(model.kind.thing) isn't saved for this site."
        case .firstTime:
            "First time on this site. Check the address is right."
        case .lookalike(_, let known):
            "Used before on \(ListFormatter.localizedString(byJoining: known))."
        case .urlDiffers:
            // Without a Face ID copy to fill from, Fill opens it again.
            model.opensWithKey(entry) ? "Fill needs your key again." : ""
        }
    }

    private var symbol: String? {
        switch tone {
        case .neutral: nil
        case .warning: PawSymbol.warning
        case .danger: PawSymbol.error
        }
    }

    private var toneColor: Color {
        switch tone {
        case .neutral: PawColor.label
        case .warning: PawColor.warning
        case .danger: PawColor.danger
        }
    }

    private var spoken: String {
        let prefix = switch tone {
        case .neutral: ""
        case .warning, .danger: "Warning: "
        }
        return prefix + spokenSentences(headline, line)
    }

    var body: some View {
        VStack(spacing: PawMetrics.Spacing.l) {
            VStack(spacing: PawMetrics.Spacing.s) {
                if let symbol {
                    Image(systemName: symbol)
                        .font(.title2)
                        .foregroundStyle(toneColor)
                        .accessibilityHidden(true)
                }
                Text(headline)
                    .font(PawFont.columnTitle)
                    .foregroundStyle(tone == .danger ? PawColor.danger : PawColor.label)
                if !line.isEmpty {
                    Text(line)
                        .font(PawFont.subheadline)
                        .foregroundStyle(PawColor.labelSecondary)
                }
            }
            .multilineTextAlignment(.center)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(spoken)
            .accessibilityIdentifier("autofill-confirm-question")
            if let address = model.site {
                // The host as iOS gave it, in full: the thing to check.
                Text(address)
                    .font(PawFont.headline.monospaced())
                    .foregroundStyle(PawColor.label)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, PawMetrics.Spacing.m)
                    .padding(.horizontal, PawMetrics.Spacing.l)
                    .background(
                        tone == .danger ? PawColor.dangerFill : PawColor.surface,
                        in: RoundedRectangle(cornerRadius: PawMetrics.cardRadius, style: .continuous))
                    .accessibilityLabel("Address \(address)")
                    .accessibilityIdentifier("autofill-confirm-address")
            }
            Button("Cancel") { model.backToList() }
                .buttonStyle(.pawPrimary)
                .accessibilityIdentifier("autofill-confirm-cancel")
            Button("Fill") { model.fillAnyway() }
                .buttonStyle(.pawSecondary)
                .accessibilityIdentifier(isFirstFill ? "autofill-use-here" : "autofill-fill-anyway")
            Spacer(minLength: 0)
        }
        .padMaxWidth(PadLayout.sheetColumn)
        .padding(PawMetrics.Spacing.l)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(PawColor.bg.ignoresSafeArea())
    }

    /// A name match, filled here for the first time or on a lookalike: the
    /// identifier the UI tests know as "Use it here".
    private var isFirstFill: Bool {
        switch reason {
        case .firstTime, .lookalike: true
        case .otherSite, .urlDiffers: false
        }
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
    let model: AutoFillModel

    var body: some View {
        KeyTaskSheet(
            driver: model.keyTask,
            purpose: model.keyEntry.map(model.purpose) ?? "",
            note: model.keepsCopies ? FaceIdWords.afterKey("fills") : nil,
            onCancel: { model.cancelKey() })
            .background(PawColor.surface.ignoresSafeArea())
    }
}
