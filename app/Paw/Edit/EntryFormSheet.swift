import SwiftUI

/// "New entry" and "Edit": name, folder, username, password with Generate,
/// website, other fields and notes. Save is in the bar, so the keyboard
/// never covers it, and needs no key: the footer says who will be able to
/// open the entry. Cancel with something typed asks inline first.
struct EntryFormSheet: View {
    let model: AppModel
    let edits: StoreEdits
    let purpose: EntryFormPurpose

    @State private var form: EntryFormModel?
    @State private var openProblem: String?

    var body: some View {
        NavigationStack {
            Group {
                if let form {
                    EntryFormView(model: model, edits: edits, form: form)
                } else if let openProblem {
                    List {
                        ErrorRow(kind: .error, problem: "Can't edit this now", nextStep: openProblem)
                            .pawListRow()
                    }
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("Close") { edits.form = nil }
                                .padShortcut(.cancelAction)
                                .accessibilityIdentifier("form-cancel")
                        }
                    }
                } else {
                    PawColor.bg
                }
            }
        }
        .interactiveDismissDisabled(form?.hasChanges ?? false)
        .padFormSheet()
        .onAppear(perform: makeForm)
        .onDisappear {
            form?.clear()
            form = nil
        }
        .privacyCover()
    }

    private func makeForm() {
        guard form == nil else { return }
        switch purpose {
        case .add(let folder):
            form = EntryFormModel(
                adding: folder, writer: edits.writer, folders: model.allFolders, live: model.storeSession.live)
        case .edit(let id, let focusName):
            guard let session = model.session(id) else {
                openProblem = "It was closed. Open it again, then tap Edit."
                return
            }
            do {
                form = try EntryFormModel(
                    editing: session, focusName: focusName, writer: edits.writer, folders: model.allFolders,
                    live: model.storeSession.live)
            } catch let error as WriteError {
                openProblem = editProblem(error, folder: session.ref.folder).nextStep
            } catch {
                openProblem = describe(error)
            }
        }
    }
}

private enum FormFocus: Hashable {
    case name, newFolder, username, password, website, notes, setupKey
    case fieldKey(UUID), fieldValue(UUID)
}

private struct EntryFormView: View {
    let model: AppModel
    let edits: StoreEdits
    @Bindable var form: EntryFormModel

    @FocusState private var focus: FormFocus?
    @State private var showsOptions = false
    @State private var reviewing = false

    var body: some View {
        ScrollViewReader { scroller in
            List {
                if form.confirmingDiscard {
                    Section {
                        InlineConfirmRow(
                            question: form.isEditing ? "Discard your changes?" : "Discard this new password?",
                            detail: "",
                            confirmTitle: "Discard", cancelTitle: "Keep editing", identifier: "form-discard",
                            onConfirm: close, onCancel: { form.confirmingDiscard = false })
                            .pawListRow()
                            .id("top")
                    }
                }
                nameSection
                loginSection
                websiteSection
                fieldsSection
                oneTimeCodeSection
                notesSection
                footerSection
            }
            .listStyle(.insetGrouped)
            .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle(form.isEditing ? "Edit" : "New password")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", action: cancel)
                        .padShortcut(.cancelAction)
                        .accessibilityIdentifier("form-cancel")
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save", action: save)
                        .fontWeight(.semibold)
                        .disabled(!form.canSave)
                        .accessibilityIdentifier("form-save")
                }
            }
            .navigationDestination(isPresented: $reviewing) {
                RecipientsReviewScreen(edits: edits, folder: form.effectiveFolder) {
                    reviewing = false
                    form.recipientsAccepted()
                }
            }
            .onChange(of: form.confirmingDiscard) {
                if form.confirmingDiscard {
                    focus = nil
                    withAnimation { scroller.scrollTo("top", anchor: .top) }
                }
            }
            .onChange(of: form.saveProblem) {
                if form.saveProblem != nil {
                    withAnimation { scroller.scrollTo("footer", anchor: .bottom) }
                }
            }
            .task {
                // After the sheet has come up, so the keyboard rises once.
                try? await Task.sleep(for: .milliseconds(500))
                if form.startsOnName {
                    focus = .name
                }
            }
        }
    }

    // MARK: Sections

    private var nameSection: some View {
        Section {
            FormTextRow(
                label: "Name", prompt: "Required, e.g. github.com", text: form.binding(\.name, field: "name"),
                identifier: "form-name", problem: shown(.name, "name"))
                .focused($focus, equals: .name)
                .submitLabel(.next)
                .onSubmit { focus = .username }
            folderRow
            if form.newFolder != nil {
                FormTextRow(
                    label: "New folder", prompt: "e.g. shopping or work/clients",
                    text: newFolderBinding, identifier: "form-new-folder", problem: shown(.folder, "folder"))
                    .focused($focus, equals: .newFolder)
            } else if let problem = shown(.folder, "folder") {
                FieldProblem(message: problem, identifier: "problem-folder").pawListRow()
            }
        }
    }

    private var newFolderBinding: Binding<String> {
        Binding(
            get: { form.newFolder ?? "" },
            set: { value in
                guard value != form.newFolder else { return }
                form.newFolder = value
                form.changed()
            })
    }

    private var folderRow: some View {
        Menu {
            Button {
                form.chooseFolder("")
            } label: {
                FolderChoiceLabel(title: "Not in a folder", chosen: form.newFolder == nil && form.folder.isEmpty)
            }
            ForEach(form.folders, id: \.self) { folder in
                // Not known yet: offered, and Save waits for its answer.
                let writable = form.folderOpenable[folder] ?? true
                Button {
                    form.chooseFolder(folder)
                } label: {
                    if writable {
                        FolderChoiceLabel(title: folder, chosen: form.newFolder == nil && form.folder == folder)
                    } else {
                        Text(folder)
                        Text("Needs a key paw doesn't have")
                    }
                }
                .disabled(!writable)
            }
            Divider()
            Button {
                form.chooseNewFolder()
                focus = .newFolder
            } label: {
                Label("New folder", systemImage: "folder.badge.plus")
            }
        } label: {
            HStack(spacing: PawMetrics.Spacing.s) {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                    Text("Folder")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                    Text(folderTitle)
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.label)
                        .multilineTextAlignment(.leading)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Image(systemName: "chevron.up.chevron.down")
                    .font(PawFont.footnote.weight(.semibold))
                    .foregroundStyle(PawColor.labelTertiary)
                    .accessibilityHidden(true)
            }
            .padding(.vertical, PawMetrics.Spacing.s)
            .frame(minHeight: PawMetrics.rowField)
            .contentShape(Rectangle())
        }
        .accessibilityLabel("Folder")
        .accessibilityValue(folderTitle)
        .accessibilityIdentifier("form-folder")
        .pawListRow()
    }

    private var folderTitle: String {
        if form.newFolder != nil { return "New folder" }
        return form.folder.isEmpty ? "Not in a folder" : form.folder
    }

    private var loginSection: some View {
        Section {
            FormTextRow(
                label: "Username", prompt: "Optional", text: form.binding(\.username, field: "username"),
                identifier: "form-username", problem: shown(.username, "username"))
                .focused($focus, equals: .username)
                .submitLabel(.next)
                .onSubmit { focus = .password }
            PasswordEditRow(form: form, focus: $focus, problem: shown(.password, "password"))
            HStack(spacing: PawMetrics.Spacing.s) {
                Button {
                    form.generate()
                } label: {
                    Label("Generate", systemImage: "wand.and.stars")
                }
                .buttonStyle(.pawSecondary)
                .accessibilityLabel("Generate a password")
                .accessibilityIdentifier("form-generate")
            }
            .padding(.vertical, PawMetrics.Spacing.s)
            .listRowSeparator(.hidden)
            .pawListRow()
            Button {
                withAnimation { showsOptions.toggle() }
            } label: {
                HStack(spacing: PawMetrics.Spacing.s) {
                    VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                        Text("Generator options")
                            .font(PawFont.body)
                            .foregroundStyle(PawColor.label)
                        Text("\(form.policy.length) characters \u{00B7} \(form.strengthWords.lowercased())")
                            .font(PawFont.footnote)
                            .foregroundStyle(PawColor.labelSecondary)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    Image(systemName: PawSymbol.disclosure)
                        .font(PawFont.footnote.weight(.semibold))
                        .foregroundStyle(PawColor.labelTertiary)
                        .rotationEffect(.degrees(showsOptions ? 90 : 0))
                        .accessibilityHidden(true)
                }
                .padding(.vertical, PawMetrics.Spacing.s)
                .frame(minHeight: PawMetrics.rowTwoLine)
                .contentShape(Rectangle())
            }
            .accessibilityValue(showsOptions ? "shown" : "hidden")
            .accessibilityHint("Length, symbols and look-alikes")
            .accessibilityIdentifier("form-options")
            .pawListRow()
            if showsOptions {
                GeneratorOptions(form: form)
                    .pawListRow()
            }
        }
    }

    private var websiteSection: some View {
        Section {
            FormTextRow(
                label: "Website", prompt: "e.g. https://github.com/login",
                text: form.binding(\.website, field: "website"),
                identifier: "form-website", problem: shown(.url, "website"), keyboard: .URL)
                .focused($focus, equals: .website)
        } footer: {
            Text("AutoFill uses it to match the page.")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
        }
    }

    private var fieldsSection: some View {
        Section {
            ForEach(Array(form.extraFields.enumerated()), id: \.element.id) { index, field in
                ExtraFieldRow(
                    form: form, field: field, index: index, focus: $focus,
                    problem: form.fieldProblems[field.id] ?? shown(.field(key: field.key), "field-\(field.id)"))
            }
            Button {
                form.addField()
                if let last = form.extraFields.last {
                    focus = .fieldKey(last.id)
                }
            } label: {
                Label("Add a field", systemImage: PawSymbol.add)
                    .font(PawFont.body)
                    .foregroundStyle(PawColor.label)
                    .frame(maxWidth: .infinity, minHeight: PawMetrics.tapMin, alignment: .leading)
            }
            .accessibilityIdentifier("form-add-field")
            .pawListRow()
        } header: {
            PawSectionHeader("Other fields")
        } footer: {
            Text("Saved as lines like \u{201C}pin: 1234\u{201D}.")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
        }
    }

    /// What saving does to the code the entry has: a key typed replaces it.
    private var otpState: String {
        if !form.setupKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return "Replaced when you save"
        }
        return form.removingOneTimeCode ? "Removed when you save" : "Has a one-time code"
    }

    /// The setup key, and Remove for a code the entry already has.
    private var oneTimeCodeSection: some View {
        Section {
            if form.hadOneTimeCode {
                HStack(spacing: PawMetrics.Spacing.s) {
                    Text(otpState)
                        .font(PawFont.body)
                        .foregroundStyle(form.removingOneTimeCode ? PawColor.labelSecondary : PawColor.label)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityIdentifier("form-otp-state")
                    if form.removingOneTimeCode {
                        Button("Keep") { form.setRemovingOneTimeCode(false) }
                            .font(PawFont.body.weight(.semibold))
                            .foregroundStyle(PawColor.label)
                            .buttonStyle(.borderless)
                            .frame(minWidth: PawMetrics.tapMin, minHeight: PawMetrics.tapMin)
                            .accessibilityLabel("Keep the one-time code")
                            .accessibilityIdentifier("form-otp-keep")
                    } else {
                        PawIconButton(
                            symbol: "minus.circle", label: "Remove the one-time code",
                            identifier: "form-otp-remove") {
                                form.setRemovingOneTimeCode(true)
                            }
                    }
                }
                .frame(minHeight: PawMetrics.tapMin)
                .pawListRow()
            }
            FormTextRow(
                label: "Setup key", prompt: "Key or otpauth:// address",
                text: form.binding(\.setupKey, field: "setup-key"),
                identifier: "form-setup-key", problem: shown(.oneTimeCode, "setup-key"))
                .focused($focus, equals: .setupKey)
        } header: {
            PawSectionHeader("One-time code")
        }
    }

    private var notesSection: some View {
        Section {
            TextField("Optional", text: form.binding(\.notes, field: "notes"), axis: .vertical)
                .font(PawFont.body)
                .lineLimit(3...12)
                .focused($focus, equals: .notes)
                .autocorrectionDisabled()
                .padding(.vertical, PawMetrics.Spacing.s)
                .accessibilityLabel("Notes")
                .accessibilityIdentifier("form-notes")
                .pawListRow()
            if let problem = shown(.notes, "notes") {
                FieldProblem(message: problem, identifier: "problem-notes").pawListRow()
            }
        } header: {
            PawSectionHeader("Notes")
        }
    }

    @ViewBuilder private var footerSection: some View {
        Section {
            if let problem = form.saveProblem {
                ErrorRow(
                    kind: .error, problem: problem.problem, nextStep: problem.nextStep,
                    actionTitle: problem.reviewFolder == nil ? nil : "Review",
                    action: { reviewing = true })
                    .pawListRow()
                    .id("footer")
            } else {
                RecipientsFooter(recipients: form.recipients, folder: form.effectiveFolder) {
                    reviewing = true
                }
                .pawListRow()
            }
        }
    }

    /// The core's message for `field`, once the person has typed in it.
    private func shown(_ field: DraftField, _ key: String) -> String? {
        guard form.isTouched(key) || form.isTouched(EntryFormModel.touchKey(field)) else { return nil }
        return form.problem(for: field)
    }

    // MARK: Actions

    private func cancel() {
        if form.hasChanges && !form.confirmingDiscard {
            form.confirmingDiscard = true
        } else {
            close()
        }
    }

    private func close() {
        focus = nil
        edits.form = nil
    }

    private func save() {
        focus = nil
        guard model.save(form) else { return }
        edits.form = nil
    }
}

/// A folder in the picker's menu, with a check on the chosen one.
private struct FolderChoiceLabel: View {
    let title: String
    let chosen: Bool

    var body: some View {
        if chosen {
            Label(title, systemImage: "checkmark")
        } else {
            Text(title)
        }
    }
}

/// A label over a text field, with the core's message under it.
private struct FormTextRow: View {
    let label: String
    let prompt: String
    @Binding var text: String
    let identifier: String
    let problem: String?
    var keyboard: UIKeyboardType = .default

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
            Text(label)
                .font(PawFont.footnote)
                .foregroundStyle(problem == nil ? PawColor.labelSecondary : PawColor.danger)
                .accessibilityHidden(true)
            TextField(prompt, text: $text)
                .font(PawFont.body)
                .keyboardType(keyboard)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .accessibilityLabel(label)
                .accessibilityIdentifier(identifier)
            if let problem {
                FieldProblem(message: problem, identifier: "problem-\(identifier.replacingOccurrences(of: "form-", with: ""))")
            }
        }
        .padding(.vertical, PawMetrics.Spacing.s)
        .frame(minHeight: PawMetrics.rowField)
        .pawListRow()
    }
}

/// The password: dots or plain text, Show, and the core's message.
private struct PasswordEditRow: View {
    @Bindable var form: EntryFormModel
    var focus: FocusState<FormFocus?>.Binding
    let problem: String?

    var body: some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                Text("Password")
                    .font(PawFont.footnote)
                    .foregroundStyle(problem == nil ? PawColor.labelSecondary : PawColor.danger)
                    .accessibilityHidden(true)
                Group {
                    if form.showsPassword {
                        TextField("Type one or generate it", text: form.binding(\.password, field: "password"), axis: .vertical)
                            .font(PawFont.password)
                    } else {
                        SecureField("Type one or generate it", text: form.binding(\.password, field: "password"))
                            .font(PawFont.body)
                    }
                }
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .focused(focus, equals: .password)
                .accessibilityLabel("Password")
                .accessibilityIdentifier("form-password")
                if let problem {
                    FieldProblem(message: problem, identifier: "problem-password")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            PawIconButton(
                symbol: form.showsPassword ? PawSymbol.hide : PawSymbol.reveal,
                label: form.showsPassword ? "Hide password" : "Show password",
                identifier: "form-password-reveal") {
                    form.showsPassword.toggle()
                }
        }
        .padding(.vertical, PawMetrics.Spacing.s)
        .frame(minHeight: PawMetrics.rowField)
        .pawListRow()
    }
}

/// Length, symbols, look-alikes, and how strong that makes a password.
private struct GeneratorOptions: View {
    @Bindable var form: EntryFormModel

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.m) {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.xs) {
                HStack {
                    Text("Length")
                        .font(PawFont.body)
                    Spacer()
                    Text("\(form.policy.length) characters")
                        .font(PawFont.body.monospacedDigit())
                        .foregroundStyle(PawColor.labelSecondary)
                        .accessibilityIdentifier("form-length-value")
                }
                .accessibilityHidden(true)
                Slider(value: form.lengthBinding, in: 8...64, step: 1)
                    .tint(PawColor.label)
                    .accessibilityLabel("Length")
                    .accessibilityValue("\(form.policy.length) characters")
                    .accessibilityIdentifier("form-length")
            }
            Toggle("Symbols (! # % + - . = ? @ _)", isOn: form.toggleBinding(\.symbols))
                .font(PawFont.body)
                .accessibilityIdentifier("form-symbols")
            Toggle("Leave out look-alikes (0 O 1 l I o)", isOn: form.toggleBinding(\.excludeLookAlikes))
                .font(PawFont.body)
                .accessibilityIdentifier("form-look-alikes")
            Text("Strength: \(form.strengthWords.lowercased())")
                .font(PawFont.subheadline.weight(.semibold))
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("form-strength")
            Text("Used the next time you generate.")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
        }
        .padding(.vertical, PawMetrics.Spacing.s)
    }
}

/// One `key: value` line: two fields and a remove button.
private struct ExtraFieldRow: View {
    @Bindable var form: EntryFormModel
    let field: EntryFormModel.ExtraField
    let index: Int
    var focus: FocusState<FormFocus?>.Binding
    let problem: String?

    var body: some View {
        HStack(alignment: .top, spacing: PawMetrics.Spacing.s) {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.xs) {
                TextField("Name, e.g. pin", text: form.fieldBinding(field.id, key: true))
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .focused(focus, equals: .fieldKey(field.id))
                    .accessibilityLabel("Field \(index + 1) name")
                    .accessibilityIdentifier("form-field-key-\(index)")
                Group {
                    if field.startsHidden {
                        SecureField("Value", text: form.fieldBinding(field.id, key: false))
                    } else {
                        TextField("Value", text: form.fieldBinding(field.id, key: false))
                    }
                }
                .font(PawFont.body)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .focused(focus, equals: .fieldValue(field.id))
                .accessibilityLabel(field.key.isEmpty ? "Field \(index + 1) value" : field.key)
                .accessibilityIdentifier("form-field-value-\(index)")
                if let problem {
                    FieldProblem(message: problem, identifier: "problem-field-\(index)")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            PawIconButton(
                symbol: "minus.circle", label: "Remove \(field.key.isEmpty ? "this field" : field.key)",
                identifier: "form-field-remove-\(index)") {
                    form.removeField(field.id)
                }
        }
        .padding(.vertical, PawMetrics.Spacing.s)
        .pawListRow()
    }
}

/// The core's message about one field, in the danger colour with its glyph.
struct FieldProblem: View {
    let message: String
    let identifier: String

    var body: some View {
        Label {
            Text(message)
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.danger)
                .fixedSize(horizontal: false, vertical: true)
        } icon: {
            Image(systemName: PawSymbol.error)
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.danger)
        }
        .labelStyle(.titleAndIcon)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Problem: \(message)")
        .accessibilityIdentifier(identifier)
    }
}

extension StoreWriter {
    /// Whether paw has every key `folder` is encrypted for. A folder whose
    /// list of keys only changed stays choosable: the form then offers the
    /// review.
    func folderOpenable(_ folder: String) -> Bool {
        guard let recipients = try? folderRecipients(folder: folder) else { return false }
        return recipients.lines.allSatisfy(\.isReady)
    }
}
