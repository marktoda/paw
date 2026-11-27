import SwiftUI
import UIKit

/// An opened entry. Values come from the Rust side one at a time, when
/// they are shown or copied; secrets start hidden and hide again after 30
/// seconds, when the app leaves the foreground and when the screen goes.
struct EntryScreen: View {
    let model: AppModel
    private let opened: OpenSession

    init(model: AppModel, session: OpenSession) {
        self.model = model
        opened = session
    }

    /// The session as it is now: a save replaces its text under the same id.
    private var session: OpenSession {
        model.session(opened.id) ?? opened
    }

    @Environment(\.scenePhase) private var scenePhase
    @State private var username: String?
    @State private var website: String?
    @State private var notes: String?
    /// Values of the fields that are not secret, fetched to be shown.
    @State private var plainValues: [UInt32: String] = [:]
    /// Why the one-time-code line can't make codes; `nil` if it can.
    @State private var otpProblem: String?
    @State private var passwordRevealed = false
    /// Bumped to hide every revealed secret at once.
    @State private var hideToken = 0

    private var view: EntryView { session.view }
    private var entry: EntryRef { session.ref }

    var body: some View {
        List {
            EntryDeleteConfirm(model: model, session: session)
            Section {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.l) {
                    header
                    if let copy = model.copies.copy(of: entry.path) {
                        // This entry's copy only; Lock now in Settings ends them all.
                        FaceIDWindowBar(
                            text: "Opens with Face ID until \(clockTime(ms: copy.untilMs))",
                            lockTitle: "Lock",
                            lockLabel: "Lock this password"
                        ) {
                            model.lockEntry(session)
                        }
                    }
                }
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            }
            fields
            if view.hasPassword {
                Section {
                    VStack(spacing: PawMetrics.Spacing.s) {
                        Button("Copy password") { copy(session.entry.password(), what: "Password") }
                            .buttonStyle(.pawPrimary)
                            // ⌘C on iPad: this button, so only once the entry is
                            // open, and never while a sheet covers it.
                            .padShortcut(KeyboardShortcut("c", modifiers: .command), enabled: !model.sheetIsUp)
                            .accessibilityIdentifier("copy-password")
                        Button(passwordRevealed ? "Hide password" : "Show password") {
                            passwordRevealed.toggle()
                        }
                        .buttonStyle(.pawSecondary)
                        .accessibilityIdentifier("show-password")
                    }
                    .listRowInsets(EdgeInsets())
                    .listRowBackground(Color.clear)
                }
            }
            Section {
                if let notes, !notes.isEmpty {
                    Text(notes)
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.label)
                        .textSelection(.enabled)
                        .accessibilityIdentifier("entry-notes")
                }
            } header: {
                if notes?.isEmpty == false {
                    PawSectionHeader("Notes")
                }
            } footer: {
                if let footer {
                    Text(footer)
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
            }
            #if DEBUG && targetEnvironment(simulator)
            // Stand-ins for the iPhone locking and time passing while
            // this entry is on screen: one dashed strip under the rest.
            Section {
                SimulatorStrip(actions: [
                    SimulatorAction(title: "lock the iPhone", identifier: "sim-entry-device-lock") {
                        NotificationCenter.default.post(
                            name: UIApplication.protectedDataWillBecomeUnavailableNotification, object: nil)
                    },
                    SimulatorAction(title: "9 hours on", identifier: "sim-entry-advance-9h") {
                        SimulatedClockHolder.shared.advance(hours: 9)
                        model.timePassed()
                    },
                ])
                .frame(maxWidth: .infinity)
                .listRowInsets(EdgeInsets())
            }
            .listRowBackground(Color.clear)
            #endif
        }
        .listStyle(.insetGrouped)
        .contentMargins(.top, 0, for: .scrollContent)
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .navigationBarTitleDisplayMode(.inline)
        .sheetOrPadCover(item: largeType) { item in
            LargeTypeSheet(item: item) { closeLargeType() }
        }
        .onAppear { load() }
        .onDisappear {
            // The entry closed (another was opened, or its folder left):
            // its large type goes with it.
            closeLargeType()
        }
        .onChange(of: model.session(opened.id) == nil) {
            if model.session(opened.id) == nil { closeLargeType() }
        }
        .onChange(of: ObjectIdentifier(session.entry)) { load() }
        .entryEditing(model, session: session)
        .onChange(of: scenePhase) {
            // Inactive or background: revealed values mask again and Large
            // Type closes, so the app switcher and the return show neither.
            if scenePhase != .active {
                hideAll()
            }
        }
    }

    /// This entry's large type: another entry's stays with that entry.
    private var largeType: Binding<LargeTypeItem?> {
        Binding(
            get: { model.largeType?.entry == opened.id ? model.largeType : nil },
            set: { item in
                if item == nil { closeLargeType() }
            })
    }

    private func closeLargeType() {
        if model.largeType?.entry == opened.id {
            model.largeType = nil
        }
    }

    // MARK: Parts

    private var header: some View {
        HStack(spacing: 14) {
            IconChip(symbol: EntryGlyph.symbol(for: entry.name), tint: FolderTint(folder: entry.topFolder), size: .large)
            VStack(alignment: .leading, spacing: 0) {
                Text(entry.name)
                    .font(PawFont.title)
                    .foregroundStyle(PawColor.label)
                    .lineLimit(3)
                    .accessibilityAddTraits(.isHeader)
                    .accessibilityIdentifier("entry-title")
                Text(entry.folder.isEmpty ? "Not in a folder" : entry.folder)
                    .font(PawFont.subheadline)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("entry-folder")
            }
        }
        .padding(.horizontal, PawMetrics.Spacing.xs)
    }

    @ViewBuilder private var fields: some View {
        Section {
            if let username {
                FieldRow(label: "Username", value: username) {
                    copy(session.entry.username(), what: "Username")
                }
                .pawListRow()
            }
            if view.hasPassword {
                SecretRow(
                    label: "Password", identifier: "password", hideToken: hideToken,
                    isRevealed: $passwordRevealed,
                    fetch: { session.entry.password() },
                    onCopy: { copy(session.entry.password(), what: "Password") },
                    onLargeType: { model.largeType = LargeTypeItem(entry: opened.id, title: "Password", text: $0) })
            }
            if let website {
                FieldRow(label: "Website", value: website) {
                    copy(session.entry.url(), what: "Website")
                }
                .pawListRow()
            }
            ForEach(view.fields, id: \.id) { field in
                let label = Self.label(field.key)
                if field.startsHidden {
                    HiddenFieldRow(
                        label: label, identifier: "field-\(field.id)", hideToken: hideToken,
                        fetch: { session.entry.fieldValue(id: field.id) },
                        onCopy: { copy(session.entry.fieldValue(id: field.id), what: label) },
                        onLargeType: { model.largeType = LargeTypeItem(entry: opened.id, title: label, text: $0) })
                } else {
                    FieldRow(label: label, value: plainValues[field.id] ?? "") {
                        copy(session.entry.fieldValue(id: field.id), what: label)
                    }
                    .pawListRow()
                }
            }
            if view.hasOtp {
                if let otpProblem {
                    VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                        Text("One-time code")
                            .font(PawFont.footnote)
                            .foregroundStyle(PawColor.labelSecondary)
                        Text(otpProblem)
                            .font(PawFont.body)
                            .foregroundStyle(PawColor.labelSecondary)
                    }
                    .padding(.vertical, PawMetrics.Spacing.s)
                    .frame(maxWidth: .infinity, minHeight: PawMetrics.rowField, alignment: .leading)
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("field-one-time-code")
                    .pawListRow()
                } else if model.session(opened.id) != nil {
                    // Not once the entry has closed: its text is gone.
                    OneTimeCodeRow(entry: session.entry, clock: model.storeSession.clock, ticking: scenePhase == .active) {
                        copy($0, what: "One-time code")
                    }
                    .pawListRow()
                }
            }
        }
    }

    /// The copy confirmation says when the clipboard clears; said once, there.
    private var footer: String? {
        model.copies.copy(of: entry.path) == nil ? "Needs your key each time." : nil
    }

    // MARK: Values

    private func load() {
        username = view.hasUsername ? session.entry.username() : nil
        website = view.hasUrl ? session.entry.url() : nil
        notes = view.hasNotes ? session.entry.notes() : nil
        var values: [UInt32: String] = [:]
        for field in view.fields where !field.startsHidden {
            values[field.id] = session.entry.fieldValue(id: field.id)
        }
        plainValues = values
        otpProblem = view.hasOtp ? session.entry.otpProblem() : nil
    }

    private func hideAll() {
        passwordRevealed = false
        hideToken += 1
        closeLargeType()
    }

    private func copy(_ value: String?, what: String) {
        guard let value else { return }
        model.storeSession.clipboard.copy(value)
        model.toast = Toast(text: "\(what) copied. \(clearsIn)", kind: .copied)
    }

    /// "security answer" reads as "Security answer".
    private static func label(_ key: String) -> String {
        key.prefix(1).uppercased() + key.dropFirst()
    }
}

/// The entry's one-time code, worked out from the open entry every second
/// while paw is active: neither the code nor its secret is kept anywhere
/// else, and it stops when the entry closes.
private struct OneTimeCodeRow: View {
    let entry: OpenedEntry
    let clock: Clock
    /// `false` under the privacy cover: no new code each second.
    let ticking: Bool
    let onCopy: (String?) -> Void

    var body: some View {
        if ticking {
            TimelineView(.periodic(from: .now, by: 1)) { _ in
                row
            }
        } else {
            row
        }
    }

    @ViewBuilder private var row: some View {
        if let code = entry.otpCode(nowMs: clock.wallMs()) {
            let shown = Self.grouped(code.code)
            HStack(spacing: PawMetrics.Spacing.xs) {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                    Text("One-time code")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                    Text(shown)
                        .font(PawFont.password)
                        .foregroundStyle(PawColor.label)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("One-time code")
                // Read digit by digit; UI tests still see "123 456".
                .accessibilityValue(Text(shown).speechSpellsOutCharacters())
                // The seconds only when asked, so the row isn't read again
                // each second.
                .accessibilityCustomContent(Text("Time left"), Text(Self.secondsLeft(code.secondsLeft)))
                .accessibilityIdentifier("field-one-time-code")
                CountdownRing(secondsLeft: code.secondsLeft, period: code.period)
                PawIconButton(
                    symbol: PawSymbol.copy, label: "Copy one-time code",
                    identifier: "copy-one-time-code"
                ) {
                    // The code as it is at the tap, not as last drawn.
                    onCopy(entry.otpCode(nowMs: clock.wallMs())?.code)
                }
            }
            .padding(.vertical, PawMetrics.Spacing.s)
            .frame(minHeight: PawMetrics.rowField)
        }
    }

    /// "1 second", "12 seconds".
    static func secondsLeft(_ seconds: UInt32) -> String {
        counted(Int(seconds), "second", "seconds")
    }

    /// "123456" reads as "123 456", "12345678" as "1234 5678".
    static func grouped(_ code: String) -> String {
        let half = code.count / 2
        return String(code.prefix(half)) + " " + String(code.dropFirst(half))
    }
}

/// Seconds left on the code, as a thin ring that empties.
private struct CountdownRing: View {
    let secondsLeft: UInt32
    let period: UInt32

    @ScaledMetric private var size: CGFloat = 18

    var body: some View {
        ZStack {
            Circle()
                .stroke(PawColor.fill, lineWidth: 2)
            Circle()
                .trim(from: 0, to: CGFloat(secondsLeft) / CGFloat(max(period, 1)))
                .stroke(PawColor.labelSecondary, style: StrokeStyle(lineWidth: 2, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
        .frame(width: size, height: size)
        // Said as the code row's "Time left", only when asked.
        .accessibilityHidden(true)
    }
}

/// A secret shown as 16 dots until revealed. The value is fetched from the
/// opened entry only while it is shown, and hidden again after 30 seconds.
private struct SecretRow: View {
    let label: String
    let identifier: String
    let hideToken: Int
    @Binding var isRevealed: Bool
    let fetch: () -> String?
    let onCopy: () -> Void
    let onLargeType: (String) -> Void

    @State private var value = ""

    var body: some View {
        PasswordField(
            label: label, identifier: identifier, password: value, isRevealed: $isRevealed,
            onCopy: onCopy,
            onLargeType: {
                if let text = fetch() { onLargeType(text) }
            })
            .pawListRow()
            .onChange(of: isRevealed, initial: true) {
                value = isRevealed ? (fetch() ?? "") : ""
            }
            .onChange(of: hideToken) {
                isRevealed = false
            }
            .task(id: isRevealed) {
                guard isRevealed else { return }
                try? await Task.sleep(for: .seconds(30))
                if !Task.isCancelled { isRevealed = false }
            }
    }
}

/// A field whose key suggests a secret (a PIN, a security answer).
private struct HiddenFieldRow: View {
    let label: String
    let identifier: String
    let hideToken: Int
    let fetch: () -> String?
    let onCopy: () -> Void
    let onLargeType: (String) -> Void

    @State private var isRevealed = false

    var body: some View {
        SecretRow(
            label: label, identifier: identifier, hideToken: hideToken, isRevealed: $isRevealed,
            fetch: fetch, onCopy: onCopy, onLargeType: onLargeType)
    }
}

struct LargeTypeItem: Identifiable {
    let id = UUID()
    /// The open entry it is from (`OpenSession.id`).
    let entry: UUID
    let title: String
    let text: String
}

private struct LargeTypeSheet: View {
    let item: LargeTypeItem
    let close: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            ZStack {
                Text(item.title)
                    .font(PawFont.headline)
                    .foregroundStyle(PawColor.label)
                    .accessibilityAddTraits(.isHeader)
                HStack {
                    Spacer()
                    Button("Done", action: close)
                        .font(PawFont.headline)
                        .foregroundStyle(PawColor.label)
                        .frame(minWidth: PawMetrics.tapMin, minHeight: PawMetrics.tapMin)
                        .padShortcut(.cancelAction)
                        .accessibilityIdentifier("large-type-done")
                }
            }
            .padding(.horizontal, PawMetrics.Spacing.l)
            .padding(.top, PawMetrics.Spacing.l)
            LargeTypeView(password: item.text)
        }
        .presentationDetents([.medium, .large])
        .pawSheetBackground()
        .privacyCover()
    }
}
