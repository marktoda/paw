import Foundation
import Observation
import SwiftUI

/// The add and edit form's state. What is typed goes into the core's
/// `EntryDraft` as it changes; the core says what is wrong with it and
/// decides how a save is done (add, edit, rename, move with re-encryption).
/// The typed text is dropped and the draft released when the sheet closes.
@MainActor @Observable
final class EntryFormModel {
    /// One of the other `key: value` lines.
    struct ExtraField: Identifiable {
        let id = UUID()
        var key: String
        var value: String
        /// As opened, for an edit; `nil` for a row added here.
        let originalKey: String?
        let originalValue: String?
        /// The core says the key looks secret (a PIN, an answer).
        let startsHidden: Bool
    }

    /// The values as opened (all empty for a new entry), to tell what changed.
    private struct Original {
        var name = ""
        var folder = ""
        var username = ""
        var password = ""
        var website = ""
        var notes = ""
    }

    let purpose: EntryFormPurpose
    let isEditing: Bool
    /// "Rename or move" starts with the name field focused. Add does not,
    /// so the whole form shows before the keyboard covers half of it.
    var startsOnName: Bool {
        switch purpose {
        case .add: false
        case .edit(_, let focusName): focusName
        }
    }
    /// The opened entry has a one-time-code line.
    let hadOneTimeCode: Bool
    /// Folders that exist, for the picker.
    let folders: [String]
    /// Whether paw has every key each folder is encrypted for, read off the
    /// main thread when the form opens and when the store changes. A folder
    /// not in it yet is offered: Save waits for that folder's own answer.
    private(set) var folderOpenable: [String: Bool] = [:]

    var name = ""
    /// The chosen existing folder, "" for the top.
    var folder = ""
    /// Set when "New folder" is chosen: the folder typed, `/` for nesting.
    var newFolder: String?
    var username = ""
    var password = ""
    var website = ""
    var notes = ""
    var extraFields: [ExtraField] = []
    var showsPassword = false
    /// A base32 setup key or an otpauth:// address; replaces the entry's
    /// one-time code when it is saved.
    var setupKey = ""
    /// The entry's one-time code goes when it is saved.
    private(set) var removingOneTimeCode = false

    /// The generator's options. Length and the two switches are the ones
    /// offered; the rest stay as the core's default policy has them.
    var policy: PasswordPolicy = defaultPasswordPolicy()
    /// The last password the generator made, while it is what the field holds.
    private(set) var generated: String?

    private(set) var problems: [DraftProblem] = []
    /// The core's message for an extra field it would refuse, by row.
    private(set) var fieldProblems: [UUID: String] = [:]
    /// Fields typed in, so a message is not shown before anything was typed.
    private(set) var touched: Set<String> = []
    private(set) var recipients: Result<FolderRecipients, Error>?
    /// Why the last save was refused.
    var saveProblem: EditProblem?
    var confirmingDiscard = false

    @ObservationIgnored private let writer: StoreWriter
    /// The names Home and search hold, for "already used", followed as the
    /// store changes.
    @ObservationIgnored private var store: PassStore
    /// Saved: the store changes no longer concern it.
    @ObservationIgnored private var done = false
    @ObservationIgnored private var recipientsTask: Task<Void, Never>?
    @ObservationIgnored private var openableTask: Task<Void, Never>?
    @ObservationIgnored private var draft: EntryDraft?
    @ObservationIgnored private var original = Original()
    /// Setters already called on the draft, so a value changed back is
    /// set back too.
    @ObservationIgnored private var applied: Set<String> = []
    @ObservationIgnored private var appliedFieldKeys: Set<String> = []
    /// The extra fields' keys as opened, lower-cased.
    @ObservationIgnored private var openedFieldKeys: Set<String> = []
    @ObservationIgnored private var recipientsFolder: String?
    /// The folder `recipients` is the answer for.
    @ObservationIgnored private var answeredFolder: String?

    /// A blank form in `folder`.
    init(adding folder: String, writer: StoreWriter, folders: [String], live: LiveStore) {
        purpose = .add(folder: folder)
        isEditing = false
        hadOneTimeCode = false
        self.writer = writer
        self.folders = folders
        store = live.store
        self.folder = folder
        original.folder = folder
        draft = writer.newDraft(folder: folder)
        changed()
        loadFolderOpenable()
        follow(live)
    }

    /// The open entry `session`, filled in. Throws if it was closed.
    init(
        editing session: OpenSession, focusName: Bool, writer: StoreWriter, folders: [String], live: LiveStore
    ) throws {
        purpose = .edit(sessionID: session.id, focusName: focusName)
        isEditing = true
        hadOneTimeCode = session.view.hasOtp
        self.writer = writer
        self.folders = folders
        store = live.store
        let opened = session.entry
        draft = try writer.editDraft(opened: opened, entry: session.ref.path)
        original = Original(
            name: session.ref.name, folder: session.ref.folder,
            username: opened.username() ?? "", password: opened.password() ?? "",
            website: opened.url() ?? "", notes: opened.notes() ?? "")
        name = original.name
        folder = original.folder
        username = original.username
        password = original.password
        website = original.website
        notes = original.notes
        extraFields = session.view.fields.map { field in
            let value = opened.fieldValue(id: field.id) ?? ""
            return ExtraField(
                key: field.key, value: value, originalKey: field.key, originalValue: value,
                startsHidden: field.startsHidden)
        }
        openedFieldKeys = Set(extraFields.compactMap { $0.originalKey.map(Self.fieldKey) })
        changed()
        loadFolderOpenable()
        follow(live)
    }

    private func follow(_ live: LiveStore) {
        live.changed.subscribe(self) { form, changed in
            form.storeChanged(changed.store)
        }
    }

    // MARK: Typing

    /// A binding that tells the draft about each change.
    func binding(_ keyPath: ReferenceWritableKeyPath<EntryFormModel, String>, field: String) -> Binding<String> {
        Binding(
            get: { self[keyPath: keyPath] },
            set: { value in
                // A field can be set to what it holds as it gains focus.
                guard value != self[keyPath: keyPath] else { return }
                self[keyPath: keyPath] = value
                self.touched.insert(field)
                self.changed()
            })
    }

    func fieldBinding(_ id: UUID, key: Bool) -> Binding<String> {
        Binding(
            get: { self.extraFields.first { $0.id == id }.map { key ? $0.key : $0.value } ?? "" },
            set: { value in
                guard let index = self.extraFields.firstIndex(where: { $0.id == id }) else { return }
                guard value != (key ? self.extraFields[index].key : self.extraFields[index].value) else { return }
                if key {
                    self.extraFields[index].key = value
                } else {
                    self.extraFields[index].value = value
                }
                self.touched.insert("field-\(id)")
                self.changed()
            })
    }

    func addField() {
        extraFields.append(ExtraField(key: "", value: "", originalKey: nil, originalValue: nil, startsHidden: false))
        changed()
    }

    func removeField(_ id: UUID) {
        extraFields.removeAll { $0.id == id }
        changed()
    }

    /// Remove, or keep after all, the opened entry's one-time code.
    func setRemovingOneTimeCode(_ removing: Bool) {
        removingOneTimeCode = removing
        changed()
    }

    func chooseFolder(_ folder: String) {
        self.folder = folder
        newFolder = nil
        touched.insert("folder")
        changed()
    }

    func chooseNewFolder() {
        newFolder = ""
        touched.insert("folder")
        changed()
    }

    /// Where the entry will be saved.
    var effectiveFolder: String {
        newFolder?.trimmingCharacters(in: CharacterSet(charactersIn: "/")) ?? folder
    }

    // MARK: The generator

    /// The Generate button: a new password from the options, shown.
    func generate() {
        guard let draft else { return }
        do {
            let made = try draft.generatePassword(policy: policy)
            password = made
            generated = made
            showsPassword = true
            touched.insert("password")
            changed()
        } catch let error as WriteError {
            saveProblem = editProblem(error, folder: effectiveFolder)
        } catch {
            saveProblem = EditProblem(problem: "That password can't be generated", nextStep: describe(error))
        }
    }

    /// An option changed: a generated password is made again to match.
    func policyChanged() {
        if generated != nil, password == generated {
            generate()
        }
    }

    var lengthBinding: Binding<Double> {
        Binding(
            get: { Double(self.policy.length) },
            set: { value in
                let length = UInt32(value.rounded())
                guard length != self.policy.length else { return }
                self.policy.length = length
                self.policyChanged()
            })
    }

    func toggleBinding(_ keyPath: WritableKeyPath<PasswordPolicy, Bool>) -> Binding<Bool> {
        Binding(
            get: { self.policy[keyPath: keyPath] },
            set: { value in
                self.policy[keyPath: keyPath] = value
                self.policyChanged()
            })
    }

    /// "Very strong", from the core's entropy for the options.
    var strengthWords: String {
        guard let bits = try? passwordEntropyBits(policy: policy) else { return "Can't be generated" }
        switch bits {
        case ..<40: return "Weak"
        case ..<64: return "Fair"
        case ..<100: return "Strong"
        default: return "Very strong"
        }
    }

    // MARK: State

    /// Hands every change to the draft, then asks the core what is wrong.
    func changed() {
        guard let draft else { return }
        saveProblem = nil
        draft.setName(name: name)
        draft.setFolder(folder: effectiveFolder)
        setIfChanged("username", username, original.username) { draft.setUsername(username: $0) }
        setIfChanged("website", website, original.website) { draft.setUrl(url: $0) }
        setIfChanged("notes", notes, original.notes) { draft.setNotes(notes: $0) }
        if password != generated {
            setIfChanged("password", password, original.password) { draft.setPassword(password: $0) }
        }
        applyFields(to: draft)
        // One slot in the draft, replaced each time: a key typed wins over
        // Remove, and a blank one leaves the line as it was.
        if removingOneTimeCode && setupKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            draft.removeOneTimeCode()
        } else {
            draft.setOneTimeCode(setup: setupKey)
        }
        // From the names in memory, never the store's lock. Save checks
        // again with the lock held.
        problems = draft.problems(store: store)
        if recipientsFolder != effectiveFolder {
            loadRecipients()
        }
    }

    /// Who can open the folder the entry goes in, read off the main thread:
    /// it takes the store's lock, which a sync can hold for seconds. While a
    /// new folder is typed, once the typing pauses. A late answer for a
    /// folder no longer chosen is dropped. The answer for the same folder
    /// stays until the new one lands; another folder has none until then.
    private func loadRecipients() {
        let folder = effectiveFolder
        let pause = newFolder != nil
        let writer = writer
        recipientsFolder = folder
        if answeredFolder != folder {
            answeredFolder = nil
            recipients = nil
        }
        recipientsTask?.cancel()
        recipientsTask = Task { [weak self] in
            if pause {
                try? await Task.sleep(for: .milliseconds(300))
                if Task.isCancelled { return }
            }
            let result = await Task.detached(priority: .userInitiated) {
                Result { try writer.folderRecipients(folder: folder) }
            }.value
            guard !Task.isCancelled, let self, self.recipientsFolder == folder else { return }
            self.answeredFolder = folder
            self.recipients = result
        }
    }

    /// Which folders paw can save in, for the picker, read off the main
    /// thread (each one takes the store's lock). Stops between folders once
    /// the sheet closes or the store is read again.
    private func loadFolderOpenable() {
        let folders = folders
        let writer = writer
        openableTask?.cancel()
        openableTask = Task { [weak self] in
            let work = Task.detached(priority: .userInitiated) { () -> [String: Bool] in
                var found: [String: Bool] = [:]
                for folder in folders where found[folder] == nil {
                    if Task.isCancelled { break }
                    found[folder] = writer.folderOpenable(folder)
                }
                return found
            }
            let found = await withTaskCancellationHandler {
                await work.value
            } onCancel: {
                work.cancel()
            }
            guard !Task.isCancelled, let self else { return }
            self.folderOpenable = found
        }
    }

    /// The store changed (a sync, a change undone, a folder's new list of
    /// keys accepted): check the name against the names now and read the
    /// folders' keys again.
    private func storeChanged(_ store: PassStore) {
        guard draft != nil, !done else { return }
        self.store = store
        recipientsFolder = nil
        changed()
        loadFolderOpenable()
    }

    private func setIfChanged(_ key: String, _ value: String, _ original: String, _ set: (String) -> Void) {
        if value != original || applied.contains(key) {
            applied.insert(key)
            set(value)
        }
    }

    /// Fields: a key no row has any more is removed; a row that changed
    /// is set. Unchanged rows are left alone, so their lines stay byte for
    /// byte. A row the core would refuse is kept out of the draft, whose
    /// edits only add up.
    private func applyFields(to draft: EntryDraft) {
        var refused: [UUID: String] = [:]
        let current = Set(extraFields.map { Self.fieldKey($0.key) }.filter { !$0.isEmpty })
        for key in openedFieldKeys.union(appliedFieldKeys) where !current.contains(key) {
            draft.removeField(key: key)
            appliedFieldKeys.remove(key)
        }
        for field in extraFields {
            let key = Self.fieldKey(field.key)
            if key.isEmpty && field.value.isEmpty { continue }
            let changed = field.originalKey.map(Self.fieldKey) != key || field.originalValue != field.value
            guard changed || appliedFieldKeys.contains(key) else { continue }
            if let problem = fieldProblem(key: field.key, value: field.value) {
                refused[field.id] = problem.message
                continue
            }
            appliedFieldKeys.insert(key)
            draft.setField(key: field.key, value: field.value)
        }
        fieldProblems = refused
    }

    private static func fieldKey(_ key: String) -> String {
        key.trimmingCharacters(in: .whitespaces).lowercased()
    }

    /// What the core says is wrong with `field`, shown under it once it was typed in.
    func problem(for field: DraftField) -> String? {
        problems.first { $0.field == field }?.message
    }

    /// Something was typed or changed: Cancel then asks first.
    var hasChanges: Bool {
        name != original.name || effectiveFolder != original.folder || username != original.username
            || password != original.password || website != original.website || notes != original.notes
            || extraFields.contains { field in
                field.originalKey == nil
                    ? !(field.key.isEmpty && field.value.isEmpty)
                    : field.originalKey != field.key || field.originalValue != field.value
            }
            || openedFieldKeys.count != extraFields.filter { $0.originalKey != nil }.count
            || !setupKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || removingOneTimeCode
    }

    /// Whether the folder can be saved in now; `nil` while not known or
    /// when it couldn't be told. Save waits for `true`.
    var folderWritable: Bool? {
        guard case .success(let recipients)? = recipients else { return nil }
        return recipients.writable
    }

    var canSave: Bool {
        problems.isEmpty && fieldProblems.isEmpty && (isEditing ? hasChanges : true) && folderWritable == true
    }

    /// Tries to save. Returns what was saved, or says why not in
    /// `saveProblem`.
    func save(copies: CopyCache?) -> Saved? {
        guard let draft else { return nil }
        changed()
        guard problems.isEmpty else { return nil }
        do {
            // A new entry gets a Face ID copy from the save (design 2.5,
            // owner's decision 8.1.10): the person just typed it on this
            // phone, usually to fill it straight in. An edit keeps the
            // copy's deadline, which the core handles.
            let ttl = isEditing ? 0 : FaceIdOpener.windowSecs
            let saved = try BackgroundTime.run("Save") {
                try writer.save(draft: draft, copies: copies, newCopyTtlSecs: ttl)
            }
            // Its own save must not show as a name already used while
            // the sheet closes.
            done = true
            return saved
        } catch let error as WriteError {
            if case .Invalid(let found) = error {
                problems = found
                found.forEach { touched.insert(Self.touchKey($0.field)) }
            } else if case .AlreadyExists = error {
                touched.insert("name")
                problems = [DraftProblem(field: .name, message: editProblem(error, folder: effectiveFolder).problem)]
            } else {
                saveProblem = editProblem(error, folder: effectiveFolder)
            }
            loadRecipients()
            return nil
        } catch {
            saveProblem = EditProblem(problem: "Couldn't save this", nextStep: "Nothing was changed. \(describe(error))")
            return nil
        }
    }

    static func touchKey(_ field: DraftField) -> String {
        switch field {
        case .name: "name"
        case .folder: "folder"
        case .password: "password"
        case .username: "username"
        case .url: "website"
        case .notes: "notes"
        case .field(let key): "key-\(key)"
        case .oneTimeCode: "setup-key"
        }
    }

    func isTouched(_ key: String) -> Bool {
        touched.contains(key)
    }

    /// The sheet is closing: drop the typed text and the draft, whose
    /// memory the core wipes when it is released.
    func clear() {
        name = ""
        username = ""
        password = ""
        website = ""
        notes = ""
        extraFields = []
        setupKey = ""
        removingOneTimeCode = false
        generated = nil
        original = Original()
        draft = nil
        recipientsTask?.cancel()
        openableTask?.cancel()
    }
}

extension RecipientLine {
    var isReady: Bool {
        if case .ready = status { return true }
        return false
    }
}
