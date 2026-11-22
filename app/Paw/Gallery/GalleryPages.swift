#if DEBUG
import SwiftUI
import UIKit

struct ColoursPage: View {
    private static let neutrals: [(String, Color)] = [
        ("bg", PawColor.bg), ("surface", PawColor.surface),
        ("surfaceRaised", PawColor.surfaceRaised), ("fill", PawColor.fill),
        ("separator", PawColor.separator), ("label", PawColor.label),
        ("labelSecondary", PawColor.labelSecondary), ("labelTertiary", PawColor.labelTertiary),
        ("action", PawColor.action), ("onAction", PawColor.onAction),
        ("panel", PawColor.panel), ("onPanel", PawColor.onPanel),
        ("onPanelSecondary", PawColor.onPanelSecondary),
    ]
    private static let gold: [(String, Color)] = [
        ("keyGold", PawColor.keyGold), ("keyGoldRim", PawColor.keyGoldRim),
        ("keyGoldHalo", PawColor.keyGoldHalo), ("keyGoldHaloOuter", PawColor.keyGoldHaloOuter),
    ]
    private static let status: [(String, Color)] = [
        ("success", PawColor.success), ("warning", PawColor.warning),
        ("danger", PawColor.danger), ("warningFill", PawColor.warningFill),
        ("dangerFill", PawColor.dangerFill), ("passwordDigit", PawColor.passwordDigit),
        ("passwordSymbol", PawColor.passwordSymbol),
    ]

    var body: some View {
        GalleryScroll("Colours") {
            GalleryGroup("Neutrals and ink") { grid(Self.neutrals) }
            GalleryGroup("The key gold") { grid(Self.gold) }
            GalleryGroup("Status and password characters") { grid(Self.status) }
            GalleryGroup("In use") {
                Label("Opened", systemImage: PawSymbol.synced)
                    .foregroundStyle(PawColor.success)
                Label("1 try left before the key locks.", systemImage: PawSymbol.warning)
                    .foregroundStyle(PawColor.warning)
                Label("Your key is locked.", systemImage: PawSymbol.error)
                    .foregroundStyle(PawColor.danger)
                Text("Secondary text on a surface")
                    .foregroundStyle(PawColor.labelSecondary)
                VStack(alignment: .leading, spacing: 2) {
                    Text("On panel").foregroundStyle(PawColor.onPanel)
                    Text("Second line on panel").foregroundStyle(PawColor.onPanelSecondary)
                }
                .font(PawFont.subheadline)
                .padding(PawMetrics.Spacing.m)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(PawColor.panel, in: RoundedRectangle(cornerRadius: PawMetrics.inlineRadius))
            }
        }
    }

    private func grid(_ tokens: [(String, Color)]) -> some View {
        LazyVGrid(
            columns: [GridItem(.flexible()), GridItem(.flexible())],
            alignment: .leading, spacing: PawMetrics.Spacing.s
        ) {
            ForEach(tokens, id: \.0) { name, color in
                HStack(spacing: PawMetrics.Spacing.s) {
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .fill(color)
                        .frame(width: 36, height: 28)
                        .overlay {
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .strokeBorder(PawColor.separator, lineWidth: 1)
                        }
                    Text(name)
                        .font(PawFont.caption)
                        .foregroundStyle(PawColor.label)
                        .lineLimit(1)
                        .minimumScaleFactor(0.7)
                }
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("swatch-\(name)")
            }
        }
    }
}

struct TypePage: View {
    private static let styles: [(String, Font, String)] = [
        ("homeTitle", PawFont.homeTitle, "Passwords"),
        ("title", PawFont.title, "Touch your key"),
        ("columnTitle", PawFont.columnTitle, "No passwords yet"),
        ("numeral", PawFont.numeral, "2 tries left"),
        ("headline", PawFont.headline, "Copy password"),
        ("body", PawFont.body, "Fastmail"),
        ("subheadline", PawFont.subheadline, "Touch the gold part of your key."),
        ("footnote", PawFont.footnote, "email, changed 14 November"),
        ("sectionHeader", PawFont.sectionHeader, "Recent, needs your key"),
        ("caption", PawFont.caption, "Step 2 of 5"),
        ("password", PawFont.password, "O0o 1lI 5S 8B"),
        ("passwordLarge", PawFont.passwordLarge, "O0 1lI"),
    ]

    var body: some View {
        GalleryScroll("Type") {
            GalleryGroup("Type scale") {
                ForEach(Self.styles, id: \.0) { name, font, sample in
                    VStack(alignment: .leading, spacing: 0) {
                        Text(sample)
                            .font(font)
                            .foregroundStyle(PawColor.label)
                            .accessibilityIdentifier("type-\(name)")
                        Text(name)
                            .font(PawFont.caption)
                            .foregroundStyle(PawColor.labelSecondary)
                    }
                }
            }
        }
    }
}

struct TintsPage: View {
    /// Folder names whose tint the UI tests know in advance.
    private static let known: [(id: String, folder: String)] = [
        ("bank", "bank"), ("Bank", "Bank"), ("dev", "dev"), ("email", "email"),
        ("home", "home"), ("shopping", "shopping"), ("work", "work"),
        ("travel", "travel"), ("school", "school"),
        ("cafe-composed", "caf\u{00E9}"), ("cafe-decomposed", "cafe\u{0301}"),
    ]

    var body: some View {
        GalleryScroll("Folder tints") {
            GalleryGroup("The eight tints") {
                ForEach(FolderTint.allCases, id: \.rawValue) { tint in
                    HStack(spacing: PawMetrics.Spacing.m) {
                        IconChip(symbol: PawSymbol.folder, tint: tint)
                        IconChip(symbol: PawSymbol.entry, tint: tint)
                        IconChip(symbol: "envelope.fill", tint: tint, size: .large)
                        Text(tint.name)
                            .font(PawFont.body)
                            .foregroundStyle(PawColor.label)
                            .accessibilityIdentifier("tint-\(tint.name)")
                    }
                }
            }
            GalleryGroup("Assignment by folder name") {
                ForEach(Self.known, id: \.id) { item in
                    let tint = FolderTint(folder: item.folder)
                    HStack(spacing: PawMetrics.Spacing.m) {
                        IconChip(symbol: PawSymbol.folder, tint: tint)
                        Text(item.folder)
                            .font(PawFont.body)
                            .foregroundStyle(PawColor.label)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Text(tint.name)
                            .font(PawFont.footnote)
                            .foregroundStyle(PawColor.labelSecondary)
                            .accessibilityIdentifier("tint-of-\(item.id)")
                    }
                }
                HStack(spacing: PawMetrics.Spacing.m) {
                    IconChip(symbol: PawSymbol.entry, tint: FolderTint(folder: nil))
                    Text("(no folder)")
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.label)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Text(FolderTint(folder: nil).name)
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                        .accessibilityIdentifier("tint-of-none")
                }
            }
            // The whole assignment in one line, so a UI test reads it in one query.
            Text(Self.known.map { "\($0.id) \(FolderTint(folder: $0.folder).name)" }.joined(separator: ", "))
                .font(PawFont.caption)
                .foregroundStyle(PawColor.labelSecondary)
                .accessibilityIdentifier("tint-assignments")
        }
    }
}

struct SymbolsPage: View {
    private static let names: [(concept: String, name: String)] =
        PawSymbol.all + EntryGlyph.table.map { (concept: "Entry: \($0.keyword)", name: $0.symbol) }

    /// Names the system has no symbol for. A missing symbol draws nothing,
    /// so the gallery says so in words.
    private static let missing: [String] =
        names.map(\.name).filter { UIImage(systemName: $0) == nil }

    var body: some View {
        GalleryScroll("Symbols") {
            Text(Self.missing.isEmpty ? "0 missing" : "Missing: " + Self.missing.joined(separator: ", "))
                .font(PawFont.footnote)
                .foregroundStyle(Self.missing.isEmpty ? PawColor.labelSecondary : PawColor.danger)
                .accessibilityIdentifier("symbols-missing")
            LazyVGrid(
                columns: Array(repeating: GridItem(.flexible(), alignment: .top), count: 3),
                spacing: PawMetrics.Spacing.l
            ) {
                ForEach(Array(Self.names.enumerated()), id: \.offset) { _, item in
                    VStack(spacing: PawMetrics.Spacing.xs) {
                        Image(systemName: item.name)
                            .font(.title2)
                            .foregroundStyle(PawColor.label)
                            .frame(height: 32)
                        Text(item.concept)
                            .font(PawFont.caption)
                            .foregroundStyle(PawColor.label)
                        Text(Self.missing.contains(item.name) ? "missing" : item.name)
                            .font(PawFont.caption)
                            .foregroundStyle(
                                Self.missing.contains(item.name) ? PawColor.danger : PawColor.labelSecondary)
                    }
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
                }
            }
            .padding(PawMetrics.Spacing.m)
            .background(
                PawColor.surface,
                in: RoundedRectangle(cornerRadius: PawMetrics.cardRadius, style: .continuous))
        }
    }
}

struct RowsPage: View {
    @State private var lastTapped = "Nothing tapped yet"

    var body: some View {
        List {
            Section {
                Text(lastTapped)
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("rows-last-tapped")
            }
            Section {
                GallerySample.row(GallerySample.entries[0]) { lastTapped = "Tapped GitHub" }
                    .pawListRow()
                GallerySample.row(GallerySample.entries[1]) { lastTapped = "Tapped Checking account" }
                    .pawListRow()
            } header: {
                PawSectionHeader("Open with Face ID")
            }
            Section {
                GallerySample.row(GallerySample.entries[3]) { lastTapped = "Tapped Fastmail" }
                    .pawListRow()
                EntryRow(
                    name: "A very long entry name that has to give way at the tail",
                    subtitle: "shopping", symbol: "cart.fill", tint: FolderTint(folder: "shopping"),
                    access: .needsKey)
                    .pawListRow()
                EntryRow(name: "Spare door code", subtitle: "No folder", access: .needsKey)
                    .pawListRow()
            } header: {
                PawSectionHeader("Recent, needs your key")
            }
            Section {
                GallerySample.row(GallerySample.entries[5], match: "fast")
                    .pawListRow()
                GallerySample.row(GallerySample.entries[7], match: "fast")
                    .pawListRow()
            } header: {
                PawSectionHeader("Search match for \u{201C}fast\u{201D}")
            }
            Section {
                ForEach(GallerySample.folders, id: \.name) { folder in
                    FolderRow(name: folder.name, count: folder.count) {
                        lastTapped = "Tapped folder \(folder.name)"
                    }
                    .pawListRow()
                }
                FolderRow(name: "spare", count: 1)
                    .pawListRow()
            } header: {
                PawSectionHeader("Folders")
            }
            Section {
                ErrorRow(
                    kind: .error,
                    problem: "Couldn't reach your git remote.",
                    nextStep: "Passwords on this iPhone still work. paw will try again when you are online.",
                    actionTitle: "Try again")
                ErrorRow(
                    kind: .warning,
                    problem: "A new key can now open your passwords.",
                    nextStep: "Check that you added it. If you did not, change your passwords.",
                    actionTitle: "Review keys")
            } header: {
                PawSectionHeader("Problems")
            }
        }
        .listStyle(.insetGrouped)
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .navigationTitle("Rows")
        .navigationBarTitleDisplayMode(.inline)
    }
}

struct DiscPage: View {
    var body: some View {
        GalleryScroll("Key disc") {
            GalleryGroup("Needed: still") {
                HStack(spacing: PawMetrics.Spacing.xl) {
                    KeyDisc(size: .marker).accessibilityIdentifier("disc-marker")
                    KeyDisc(size: .banner).accessibilityIdentifier("disc-banner")
                    KeyDisc(size: .hero)
                }
            }
            GalleryGroup("Waiting for a touch: breathing") {
                KeyDisc(size: .hero, phase: .waiting)
                    .frame(maxWidth: .infinity)
            }
            GalleryGroup("Locked: no gold") {
                HStack(spacing: PawMetrics.Spacing.xl) {
                    KeyDisc(size: .marker, phase: .locked)
                    KeyDisc(size: .banner, phase: .locked).accessibilityIdentifier("disc-locked")
                    KeyDisc(size: .hero, phase: .locked)
                }
            }
            GalleryGroup("On the background and on a panel", onCard: false) {
                HStack(spacing: PawMetrics.Spacing.xl) {
                    KeyDisc(size: .marker)
                    KeyDisc(size: .banner)
                    KeyDisc(size: .banner, phase: .waiting)
                }
                HStack(spacing: PawMetrics.Spacing.xl) {
                    KeyDisc(size: .marker)
                    KeyDisc(size: .banner)
                    KeyDisc(size: .banner, phase: .waiting)
                }
                .padding(PawMetrics.Spacing.l)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    PawColor.panel,
                    in: RoundedRectangle(cornerRadius: PawMetrics.cardRadius, style: .continuous))
            }
        }
    }
}

struct KeySheetPage: View {
    enum Sample: String, CaseIterable, Identifiable {
        case plugIn, pin, pinForgotten, pinLastTry, wrongPin, busy, touch, working, done, locked

        var id: String { rawValue }

        var title: String {
            switch self {
            case .plugIn: "Plug in"
            case .pin: "PIN"
            case .pinForgotten: "PIN, forgotten after 15 minutes"
            case .pinLastTry: "PIN, one try left"
            case .wrongPin: "Wrong PIN, 2 tries left"
            case .busy: "Key busy"
            case .touch: "Touch"
            case .working: "Working"
            case .done: "Done"
            case .locked: "Locked"
            }
        }

        var stage: KeySheetStage {
            switch self {
            case .plugIn: .plugIn
            case .pin: .pin(triesLeft: 3, forgotten: false)
            case .pinForgotten: .pin(triesLeft: 3, forgotten: true)
            case .pinLastTry: .pin(triesLeft: 1, forgotten: false)
            case .wrongPin: .wrongPin(triesLeft: 2)
            case .busy: .busy
            case .touch: .touch
            case .working: .working(message: "Opening Fastmail\u{2026}", progress: nil)
            case .done: .done
            case .locked: .locked
            }
        }

        var needsKeyboard: Bool {
            switch self {
            case .pin, .pinForgotten, .pinLastTry, .wrongPin: true
            default: false
            }
        }
    }

    @State private var sample = Sample.touch
    @State private var isPresented = false
    @State private var pin = ""
    @State private var detent = PresentationDetent.height(PawMetrics.keySheetHeight)

    var body: some View {
        List {
            Section {
                ForEach(Sample.allCases) { item in
                    Button(item.title) { show(item) }
                        .foregroundStyle(PawColor.label)
                        .accessibilityIdentifier("stage-\(item.rawValue)")
                }
            } header: {
                PawSectionHeader("Stages")
            } footer: {
                Text("Each opens the one key sheet at that stage. Continue on a PIN stage moves on to Touch.")
            }
        }
        .navigationTitle("Key sheet")
        .navigationBarTitleDisplayMode(.inline)
        .sheet(isPresented: $isPresented) {
            KeySheetSampleHost(sample: $sample, pin: $pin, detent: $detent, isPresented: $isPresented)
        }
    }

    private func show(_ item: Sample) {
        pin = ""
        sample = item
        detent = item.needsKeyboard ? .large : .height(PawMetrics.keySheetHeight)
        isPresented = true
    }
}

/// The sheet's content. A separate view, so it follows `sample` while the
/// sheet stays up (Continue on a PIN stage moves on to Touch in place).
private struct KeySheetSampleHost: View {
    @Binding var sample: KeySheetPage.Sample
    @Binding var pin: String
    @Binding var detent: PresentationDetent
    @Binding var isPresented: Bool

    var body: some View {
        KeySheetContent(
            purpose: "Open Fastmail",
            stage: sample.stage,
            pin: $pin,
            note: "Afterwards it opens with Face ID until 6:40 PM.",
            onContinue: {
                pin = ""
                sample = .touch
                detent = .height(PawMetrics.keySheetHeight)
            },
            onCancel: { isPresented = false })
            .presentationDetents([.height(PawMetrics.keySheetHeight), .large], selection: $detent)
            .presentationDragIndicator(.visible)
            .task(id: sample) {
                // Touch received: the sheet closes itself. The app waits one
                // second; the gallery waits four so the stage can be looked at.
                guard sample == .done else { return }
                try? await Task.sleep(for: .seconds(4))
                if !Task.isCancelled { isPresented = false }
            }
    }
}

struct StatusPage: View {
    var body: some View {
        GalleryScroll("Status") {
            GalleryGroup("Sync pill") {
                SyncPill(status: .synced, text: "Synced 2 minutes ago")
                SyncPill(status: .syncing, text: "Syncing")
                SyncPill(status: .notPushed, text: "1 change not pushed")
                SyncPill(status: .offline, text: "Offline, last checked 9:12 AM")
                SyncPill(status: .failed, text: "Your git remote refused 1 change")
            }
            GalleryGroup("Not yet pushed", onCard: false) {
                UnpushedBanner(
                    reason: .waitingForKey,
                    title: "1 change is only on this iPhone",
                    detail: "Plug in your key to send it.")
                UnpushedBanner(
                    reason: .offline,
                    title: "2 changes are only on this iPhone",
                    detail: "You are offline. paw sends them when you are back online.")
                UnpushedBanner(
                    reason: .refused,
                    title: "Your git remote refused 1 change",
                    detail: "It is still saved on this iPhone. Check that your key may write to the repository.")
            }
            GalleryGroup("Face ID window", onCard: false) {
                FaceIDWindowBar(text: "Opens with Face ID until 6:40 PM")
            }
            GalleryGroup("Copy confirmation", onCard: false) {
                CopyToast(message: "Password copied. Clears in 45 seconds.")
                CopyToast(message: "Username copied.")
            }
            GalleryGroup("Problem banners", onCard: false) {
                ErrorRow(
                    kind: .error,
                    problem: "Couldn't reach your git remote.",
                    nextStep: "Passwords on this iPhone still work.",
                    actionTitle: "Try again", isBanner: true)
                ErrorRow(
                    kind: .warning,
                    problem: "You have no second key or backup.",
                    nextStep: "If this key is lost or breaks, your passwords are gone for good.",
                    actionTitle: "Add a second key", isBanner: true)
            }
        }
    }
}

struct EmptyPage: View {
    @State private var lastTapped = "Nothing tapped yet"

    var body: some View {
        GalleryScroll("Empty states") {
            EmptyState(
                symbol: PawSymbol.entry,
                title: "No passwords yet",
                message: "Add your first password, or download the ones you already keep in pass.",
                primary: EmptyState.Action(title: "Add a password", identifier: "empty-add") {
                    lastTapped = "Tapped Add a password"
                },
                secondary: EmptyState.Action(title: "Download your store", identifier: "empty-download") {
                    lastTapped = "Tapped Download your store"
                })
            Text(lastTapped)
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
                .frame(maxWidth: .infinity)
                .accessibilityIdentifier("empty-last-tapped")
            Divider()
            EmptyState(
                symbol: PawSymbol.search,
                title: "No matches",
                message: "Search looks at names and folders. What is inside a password stays encrypted until you open it.")
        }
    }
}
#endif
