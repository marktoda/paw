import SwiftUI

/// A question asked in place, instead of an alert: one sentence, a line
/// of detail, the destructive answer and the safe one.
struct InlineConfirmRow: View {
    let question: String
    let detail: String
    let confirmTitle: String
    let cancelTitle: String
    /// `<identifier>-confirm` and `<identifier>-cancel` for the buttons.
    let identifier: String
    /// The question's own identifier, where it isn't `<identifier>-question`.
    var questionIdentifier: String? = nil
    let onConfirm: () -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.m) {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                Text(question)
                    .font(PawFont.headline)
                    .foregroundStyle(PawColor.label)
                if !detail.isEmpty {
                    Text(detail)
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
            }
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityElement(children: .combine)
            .accessibilityAddTraits(.isHeader)
            .accessibilityIdentifier(questionIdentifier ?? "\(identifier)-question")
            ViewThatFits(in: .horizontal) {
                HStack(spacing: PawMetrics.Spacing.s) { buttons }
                VStack(spacing: PawMetrics.Spacing.s) { buttons }
            }
        }
        .padding(.vertical, PawMetrics.Spacing.m)
    }

    @ViewBuilder private var buttons: some View {
        Button(role: .destructive, action: onConfirm) {
            Text(confirmTitle)
                .font(PawFont.headline)
                .foregroundStyle(PawColor.danger)
                .frame(maxWidth: .infinity, minHeight: PawMetrics.tapMin)
                .background(PawColor.dangerFill, in: Capsule())
        }
        .buttonStyle(.borderless)
        .accessibilityIdentifier("\(identifier)-confirm")
        Button(action: onCancel) {
            Text(cancelTitle)
                .font(PawFont.headline)
                .foregroundStyle(PawColor.label)
                .frame(maxWidth: .infinity, minHeight: PawMetrics.tapMin)
                .background(PawColor.fill, in: Capsule())
        }
        .buttonStyle(.borderless)
        .accessibilityIdentifier("\(identifier)-cancel")
    }
}

/// Who will be able to open the entry, in plain words, or why it can't be
/// saved in this folder and what to do about it. Never offers to save for
/// fewer keys than the folder's `.gpg-id` names.
struct RecipientsFooter: View {
    let recipients: Result<FolderRecipients, Error>?
    let folder: String
    let onReview: () -> Void

    var body: some View {
        switch recipients {
        case nil:
            EmptyView()
        case .failure(let error)?:
            ErrorRow(kind: .error, problem: "Can't tell who can open this folder", nextStep: describe(error))
        case .success(let recipients)?:
            if !recipients.confirmed {
                ErrorRow(
                    kind: .warning, problem: "paw hasn't confirmed who can open this store",
                    nextStep: "Finish setting up the store, then save.")
            } else if recipients.changed {
                ErrorRow(
                    kind: .warning,
                    problem: "Who can open \(place) changed",
                    nextStep: "Review the new list before saving here.",
                    actionTitle: "Review", action: onReview)
            } else if !recipients.lines.allSatisfy(\.isReady) {
                ErrorRow(
                    kind: .warning,
                    problem: "Can't save in \(place)",
                    nextStep: "It's also encrypted for a key paw doesn't have. Pick another folder.")
            } else {
                HStack(alignment: .firstTextBaseline, spacing: PawMetrics.Spacing.s) {
                    Image(systemName: "person.badge.key")
                        .font(PawFont.subheadline)
                        .foregroundStyle(PawColor.labelSecondary)
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                        Text(Self.sentence(recipients.lines))
                            .font(PawFont.subheadline)
                            .foregroundStyle(PawColor.label)
                        Text(Device.words("Saving needs no key. It stays on this iPhone until you send it."))
                            .font(PawFont.footnote)
                            .foregroundStyle(PawColor.labelSecondary)
                    }
                    .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.vertical, PawMetrics.Spacing.m)
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("form-recipients")
            }
        }
    }

    private var place: String {
        folder.isEmpty ? "the top of your store" : folder
    }

    /// "Encrypted for your key." or "Encrypted for your key and key 9A1C 03F2."
    static func sentence(_ lines: [RecipientLine]) -> String {
        let names = lines.map(\.spokenName)
        switch names.count {
        case 0: return "Encrypted for nobody yet."
        case 1: return "Encrypted for \(names[0])."
        default: return "Encrypted for \(names.dropLast().joined(separator: ", ")) and \(names[names.count - 1])."
        }
    }
}

extension RecipientLine {
    /// "your key", or "key 9A1C 03F2" by the last 8 hex digits.
    var spokenName: String {
        switch status {
        case .ready(_, .card, _): return "your key"
        case .ready(let fingerprint, .certificate, _): return "key \(Self.tail(fingerprint))"
        default: return "key \(Self.tail(line))"
        }
    }

    /// How this line is satisfied, in words.
    var statusWords: String {
        switch status {
        case .ready(_, .card, _): return "Your key"
        case .ready: return Device.words("A key saved on this iPhone")
        case .missing: return "paw doesn't have this key"
        case .ambiguous: return "Matches more than one saved key"
        case .expired: return "This key has expired"
        case .refused(_, _, let reason): return "Can't be used: \(reason)"
        }
    }

    static func tail(_ hex: String) -> String {
        let clean = hex.filter(\.isHexDigit).uppercased()
        guard clean.count >= 8 else { return hex }
        let last = String(clean.suffix(8))
        return "\(last.prefix(4)) \(last.suffix(4))"
    }
}

/// "Review who can open this folder": the accepted list and the list now,
/// line by line, with Accept. Accepting never drops a key: paw then
/// encrypts for everyone on the new list, or says which key it lacks.
struct RecipientsReviewScreen: View {
    let edits: StoreEdits
    let folder: String
    let onAccepted: () -> Void

    @State private var recipients: Result<FolderRecipients, Error>?
    @State private var problem: String?

    var body: some View {
        List {
            Section {
                Text("The keys that can open \(place) changed. Passwords you save here will be encrypted for the new list.")
                    .font(PawFont.subheadline)
                    .foregroundStyle(PawColor.label)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.vertical, PawMetrics.Spacing.s)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 0, leading: PawMetrics.Spacing.l, bottom: 0, trailing: PawMetrics.Spacing.l))
            }
            switch recipients {
            case .success(let found)?:
                Section {
                    if found.acceptedLines.isEmpty {
                        lineRow("No list", detail: "This folder had no list of its own.")
                    }
                    ForEach(found.acceptedLines, id: \.self) { line in
                        lineRow(Self.name(line, in: found), detail: Self.grouped(line))
                    }
                } header: {
                    PawSectionHeader("Before")
                }
                .accessibilityIdentifier("recipients-before")
                Section {
                    ForEach(found.lines, id: \.line) { line in
                        lineRow(
                            Self.name(line.line, in: found),
                            detail: Self.isYourKey(line) ? Self.grouped(line.line)
                                : "\(line.statusWords) \u{00B7} \(Self.grouped(line.line))")
                    }
                } header: {
                    PawSectionHeader("Now")
                }
                .accessibilityIdentifier("recipients-now")
            case .failure(let error)?:
                ErrorRow(kind: .error, problem: "Couldn't read the list", nextStep: describe(error))
                    .pawListRow()
            case nil:
                EmptyView()
            }
            if let problem {
                ErrorRow(kind: .error, problem: "Not accepted", nextStep: problem)
                    .pawListRow()
            }
            Section {
                Button("Accept the new list", action: accept)
                    .buttonStyle(.pawPrimary)
                    .accessibilityIdentifier("recipients-accept")
                    .listRowInsets(EdgeInsets())
                    .listRowBackground(Color.clear)
            } footer: {
                Text("Not expected? Check with whoever changed it first.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Who can open \(folder.isEmpty ? "your store" : folder)")
        .navigationBarTitleDisplayMode(.inline)
        .onAppear {
            recipients = Result { try edits.writer.folderRecipients(folder: folder) }
        }
    }

    private var place: String {
        folder.isEmpty ? "your store" : folder
    }

    private func lineRow(_ title: String, detail: String) -> some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
            Text(title)
                .font(PawFont.body)
                .foregroundStyle(PawColor.label)
            Text(detail)
                .font(PawFont.footnote.monospaced())
                .foregroundStyle(PawColor.labelSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.vertical, PawMetrics.Spacing.s)
        .accessibilityElement(children: .combine)
        .pawListRow()
    }

    /// The same name for a key in both lists: "Your key" for the plugged-in
    /// key's line wherever it appears, otherwise "Key 9A1C 03F2".
    private static func name(_ line: String, in found: FolderRecipients) -> String {
        let yours = found.lines.contains { $0.line == line && isYourKey($0) }
        return yours ? "Your key" : "Key \(RecipientLine.tail(line))"
    }

    private static func isYourKey(_ line: RecipientLine) -> Bool {
        if case .ready(_, .card, _) = line.status { return true }
        return false
    }

    /// A fingerprint in groups of four, as people read them out.
    static func grouped(_ line: String) -> String {
        let hex = line.filter { !$0.isWhitespace }
        guard hex.allSatisfy(\.isHexDigit) else { return line }
        return stride(from: 0, to: hex.count, by: 4).map { offset in
            let start = hex.index(hex.startIndex, offsetBy: offset)
            let end = hex.index(start, offsetBy: 4, limitedBy: hex.endIndex) ?? hex.endIndex
            return String(hex[start..<end])
        }.joined(separator: " ")
    }

    private func accept() {
        do {
            try edits.acceptRecipients(folder: folder)
            onAccepted()
        } catch let error as WriteError {
            problem = editProblem(error, folder: folder).nextStep
        } catch {
            problem = describe(error)
        }
    }
}

/// A toast with one action ("Deleted github.com. Undo"). Unlike the copy
/// confirmation it can be reached by VoiceOver, because it can be acted on.
struct ActionToast: View {
    let message: String
    let actionTitle: String
    let action: () -> Void

    var body: some View {
        HStack(spacing: PawMetrics.Spacing.m) {
            Text(message)
                .font(PawFont.subheadline.weight(.semibold))
                .foregroundStyle(PawColor.onPanel)
                .multilineTextAlignment(.leading)
                .accessibilityIdentifier("action-toast-message")
            Button(action: action) {
                Text(actionTitle)
                    .font(PawFont.subheadline.weight(.semibold))
                    .foregroundStyle(PawColor.onPanel)
                    .underline()
                    .frame(minWidth: PawMetrics.tapMin, minHeight: PawMetrics.tapMin)
            }
            .buttonStyle(.borderless)
            .accessibilityIdentifier("action-toast-button")
        }
        .padding(.leading, PawMetrics.Spacing.l)
        .padding(.trailing, PawMetrics.Spacing.s)
        .padding(.vertical, PawMetrics.Spacing.xs)
        .frame(minHeight: PawMetrics.tapMin)
        .background(PawColor.panel, in: Capsule())
        .accessibilityElement(children: .contain)
    }
}

/// The add and edit sheet, the Undo toast and, in the simulator, a record
/// of the last toast for UI tests. Apply once, at the root.
private struct EditingModifier: ViewModifier {
    let model: AppModel
    @State private var lastToast: String?

    private var formBinding: Binding<EntryFormPurpose?> {
        Binding(
            get: { model.edits?.form },
            set: { model.edits?.form = $0 })
    }

    func body(content: Content) -> some View {
        content
            .sheet(item: formBinding) { purpose in
                if let edits = model.edits {
                    EntryFormSheet(model: model, edits: edits, purpose: purpose)
                }
            }
            .overlay(alignment: .bottom) {
                if let offer = model.edits?.undoOffer {
                    ActionToast(message: offer.message, actionTitle: "Undo") {
                        model.undo(offer)
                    }
                    .padding(.horizontal, PawMetrics.Spacing.l)
                    .padding(.bottom, 72)
                    .transition(.opacity)
                    .task(id: offer.id) {
                        AccessibilityNotification.Announcement("\(offer.message) Undo is available.").post()
                        try? await Task.sleep(for: .seconds(8))
                        if !Task.isCancelled, model.edits?.undoOffer?.id == offer.id {
                            model.edits?.undoOffer = nil
                        }
                    }
                }
            }
            .animation(.easeOut(duration: 0.2), value: model.edits?.undoOffer)
            .onChange(of: model.toast) {
                if let toast = model.toast { lastToast = toast }
            }
            #if DEBUG && targetEnvironment(simulator)
            .background(alignment: .topLeading) {
                // The toast is not focusable, by design; UI tests read it here.
                if let lastToast {
                    Text(lastToast)
                        .font(.caption2)
                        .frame(width: 1, height: 1)
                        .opacity(0.02)
                        .accessibilityIdentifier("sim-last-app-toast")
                }
            }
            #endif
    }
}

extension View {
    /// The edit sheet and the Undo toast; see `EditingModifier`.
    func pawEditing(_ model: AppModel) -> some View {
        modifier(EditingModifier(model: model))
    }

    /// An Add button at the bottom of a folder, starting in that folder.
    func addEntryButton(_ model: AppModel, folder: String) -> some View {
        toolbar {
            // In the split view the sidebar's Add is the one Add.
            if model.edits != nil, !model.usesSplitView {
                ToolbarSpacer(.flexible, placement: .bottomBar)
                ToolbarItem(placement: .bottomBar) {
                    Button {
                        model.startAdd(folder: folder)
                    } label: {
                        Label("Add a password", systemImage: PawSymbol.add)
                    }
                    .accessibilityIdentifier("add-button")
                }
            }
        }
    }
}
