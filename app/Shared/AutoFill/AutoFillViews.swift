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
        case .list, .faceId, .loading: true
        default: false
        }
    }

    @ViewBuilder private var content: some View {
        switch model.screen {
        case .loading, .filled:
            PawColor.bg.ignoresSafeArea()
        case .list:
            AutoFillList(model: model)
        case .faceId(let entry):
            AutoFillFaceIdView(entry: entry)
        case .key:
            AutoFillKeySheet(model: model)
        case .noStore:
            EmptyState(
                symbol: PawSymbol.entry, title: "Set up paw first",
                message: "Open paw and set up your store. Then try again here.",
                primary: EmptyState.Action(title: "Close", identifier: "autofill-close") { model.cancel() })
                .background(PawColor.bg.ignoresSafeArea())
                .accessibilityIdentifier("autofill-no-store")
        case .problem(let problem, let nextStep):
            VStack(spacing: PawMetrics.Spacing.l) {
                ErrorRow(kind: .error, problem: problem, nextStep: nextStep, isBanner: true)
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
        if model.matches.isEmpty {
            Section {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                    Text(model.site.map { "Nothing here matches \($0)." } ?? "Nothing here matches this site.")
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
            Section {
                ForEach(model.matches, id: \.path) { entry in
                    row(entry)
                }
            } header: {
                PawSectionHeader(model.site.map { "For \($0)" } ?? "Suggested")
                    .accessibilityIdentifier("autofill-site-section")
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
            Section {
                ForEach(model.hits, id: \.entry.path) { hit in
                    row(hit.entry,
                        nameHighlights: hit.nameRanges.map(Self.range),
                        folderHighlights: hit.folderRanges.map(Self.range))
                }
            } header: {
                PawSectionHeader(model.hits.count == 1 ? "1 match" : "\(model.hits.count) matches")
            }
        }
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

/// Behind the system's Face ID prompt: which entry is being filled.
private struct AutoFillFaceIdView: View {
    let entry: EntryRef

    var body: some View {
        VStack(spacing: PawMetrics.Spacing.l) {
            Spacer(minLength: 0)
            Image(systemName: PawSymbol.faceID)
                .font(.system(size: PawMetrics.discHero * 0.6))
                .foregroundStyle(PawColor.labelSecondary)
                .accessibilityHidden(true)
            Text("Fill \(entry.name)")
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
            purpose: "Fill \(model.keyEntry?.name ?? "")",
            stage: model.keyStage,
            pin: $model.pinEntry,
            note: model.keepsCopies
                ? "Afterwards it fills with Face ID for \(AutoFillModel.faceIdWindowSecs / 3600) hours." : nil,
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
