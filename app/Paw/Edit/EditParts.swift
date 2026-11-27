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
            if let problem = recipients.problem?.words(folder: folder) {
                ErrorRow(
                    kind: .warning, problem: problem.problem, nextStep: problem.nextStep,
                    actionTitle: problem.reviewFolder == nil ? nil : "Review", action: onReview)
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
        case .ready: return "your key"
        default: return "key \(RecipientLineWords.ending(line))"
        }
    }

    /// How this line is satisfied, in words.
    var statusWords: String {
        switch status {
        case .ready: return "Your key"
        case .missing: return "paw doesn't have this key"
        }
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
                Text("The keys that can open \(recipientsPlace(folder)) changed. Passwords you save here will be encrypted for the new list.")
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
                        lineRow(Self.name(line, in: found), detail: RecipientLineWords.grouped(line))
                    }
                } header: {
                    PawSectionHeader("Before")
                }
                .accessibilityIdentifier("recipients-before")
                Section {
                    ForEach(found.lines, id: \.line) { line in
                        lineRow(
                            Self.name(line.line, in: found),
                            detail: line.isReady ? RecipientLineWords.grouped(line.line)
                                : "\(line.statusWords) \u{00B7} \(RecipientLineWords.grouped(line.line))")
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
        .navigationTitle("Who can open \(recipientsPlace(folder))")
        .navigationBarTitleDisplayMode(.inline)
        .onAppear {
            recipients = Result { try edits.writer.folderRecipients(folder: folder) }
        }
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
        let yours = found.lines.contains { $0.line == line && $0.isReady }
        return yours ? "Your key" : "Key \(RecipientLineWords.ending(line))"
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

/// The add and edit sheet and the Undo toast. Apply once, at the root.
private struct EditingModifier: ViewModifier {
    let model: AppModel

    private var edits: StoreEdits? {
        model.storeSession.edits
    }

    private var formBinding: Binding<EntryFormPurpose?> {
        Binding(
            get: { edits?.form },
            set: { purpose in
                if purpose == nil { edits?.formClosed() }
            })
    }

    func body(content: Content) -> some View {
        content
            .pawSheet(item: formBinding) { purpose in
                if let edits {
                    EntryFormSheet(model: model, edits: edits, purpose: purpose)
                }
            }
            .overlay(alignment: .bottom) {
                if let edits, let offer = edits.undoOffer {
                    ActionToast(message: offer.message, actionTitle: "Undo") {
                        edits.undo(offer)
                    }
                    .padding(.horizontal, PawMetrics.Spacing.l)
                    .padding(.bottom, 72)
                    .transition(.opacity)
                    .task(id: offer.id) {
                        AccessibilityNotification.Announcement("\(offer.message) Undo is available.").post()
                        try? await Task.sleep(for: .seconds(8))
                        if !Task.isCancelled {
                            edits.undoOfferEnded(offer)
                        }
                    }
                }
            }
            .animation(.easeOut(duration: 0.2), value: edits?.undoOffer)
    }
}

extension View {
    func pawEditing(_ model: AppModel) -> some View {
        modifier(EditingModifier(model: model))
    }

    /// An Add button at the bottom of a folder, starting in that folder.
    func addEntryButton(_ model: AppModel, folder: String) -> some View {
        toolbar {
            // In the split view the sidebar's Add is the one Add.
            if let edits = model.storeSession.edits, !model.usesSplitView {
                ToolbarSpacer(.flexible, placement: .bottomBar)
                ToolbarItem(placement: .bottomBar) {
                    Button {
                        edits.startAdd(folder: folder)
                    } label: {
                        Label("Add a password", systemImage: PawSymbol.add)
                    }
                    .accessibilityIdentifier("add-button")
                }
            }
        }
    }
}
