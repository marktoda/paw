import Foundation
import Observation
import SwiftUI

/// Where the navigation stack can go.
enum Route: Hashable {
    case folder(String)
    /// An opened entry, by the id of its `OpenSession`.
    case entry(UUID)
    /// An entry the plugged-in key cannot open, explained.
    case anotherKey(String)
    /// Changes saved on this iPhone and not sent yet.
    case unpushed
    /// A screen about sync: held-back changes, a refused remote, replaced
    /// history (`Sync/SyncScreens.swift`).
    case sync(SyncRoute)
}

/// An entry that is open on screen. The plaintext stays in Rust, behind
/// `entry`; `close()` wipes it when the screen goes away.
struct OpenSession: Identifiable {
    /// Kept when a save replaces the entry's text, so the screen stays.
    var id = UUID()
    let ref: EntryRef
    let entry: OpenedEntry
    let view: EntryView
    /// It had a Face ID copy when it opened: it closes when the copy ends.
    var hadCopy = false
}

/// The key sheet while it is up. `nil` in `AppModel.keyRequest` means no
/// sheet; its stage is `AppModel.keyTask`'s.
struct KeyRequest: Identifiable {
    let id = UUID()
    let entry: EntryRef
    /// The key plugged in is not the store's: the sheet shows "Replace key"
    /// as a way on that is not built yet.
    var differentKey = false
}

/// A problem shown inline at the top of the list it happened in.
struct Notice: Equatable {
    let problem: String
    let nextStep: String
    /// When set, the notice offers to open this entry with the key instead.
    var keyEntry: EntryRef? = nil
}

/// The one model of the screens: navigation, the key sheet, open entries
/// and what Home shows, over a finished `StoreSession`. Gives views plain
/// values. The PIN lives in the core's key session, never here. Card and
/// Face ID calls block for seconds, so they run off the main thread and
/// their results are published here.
@MainActor @Observable
final class AppModel {
    /// An open entry survives this long with paw in the background.
    static let openEntryBackgroundLimitMs: Int64 = 2 * 60 * 1000

    let storeSession: StoreSession
    /// The key sheet's task: opening an entry with the key.
    let keyTask: KeyTaskDriver
    let faceId: FaceIdOpener

    /// The navigation stack. Changed only through `setPath`, `show` and
    /// `leave`, which close what leaves it.
    private(set) var path: [Route] = []
    /// iPad in regular width: `path` is shown as columns (`SplitPath`).
    var usesSplitView = false
    private(set) var home: FolderListing
    private(set) var entryCount: Int
    private(set) var liveCopies: [LiveCopy] = []
    /// Entry paths opened in this run, most recent first.
    private(set) var recentPaths: [String] = []
    /// What the core says about each entry for the last key plugged in.
    private(set) var accessByPath: [String: Access] = [:]
    var keyRequest: KeyRequest?
    private(set) var sessions: [UUID: OpenSession] = [:]
    /// Entries closed while on screen (2 minutes away, the iPhone locked,
    /// the Face ID time ended), by the id their route still carries.
    var closedEntries: [UUID: ClosedEntry] = [:]
    var notice: Notice?
    /// A problem with the Face ID copies themselves, shown on Home.
    private(set) var copiesProblem: String?
    var toast: String?
    var showsSettings = false
    /// An entry's large type is up (a full-screen cover on iPad), so the
    /// keyboard shortcuts stay off.
    var largeTypeUp = false

    @ObservationIgnored private var pendingRoute: Route?
    /// Bumped by each key read: which access answer is the latest.
    @ObservationIgnored private var accessAttempt = 0
    /// Monotonic milliseconds on `clock` when paw went to the background.
    @ObservationIgnored var backgroundedAtMs: Int64?
    @ObservationIgnored var deviceLockWatch: NSObjectProtocol?

    /// `notice` is said on Home from the start.
    init(session: StoreSession, notice: Notice? = nil) {
        storeSession = session
        keyTask = KeyTaskDriver(reader: session.reader)
        faceId = FaceIdOpener()
        let store = session.live.store
        home = store.home()
        entryCount = Int(store.entryCount())
        self.notice = notice
        keyTask.owner = self
        session.reader.onRead = { [weak self] keySession in
            self?.learnAccess(keySession)
        }
        session.live.changed.subscribe(self) { model, changed in
            model.storeChanged(changed)
        }
        session.requests.subscribe(self) { model, request in
            model.asked(request)
        }
        sweepCopies()
        watchDeviceLock()
    }

    /// The store as it is now: read again after each change.
    var store: PassStore {
        storeSession.live.store
    }

    /// `nil` where Face ID copies cannot be kept: on a device until the
    /// Secure Enclave keystore exists. Every open then uses the key.
    var copies: CopyCache? {
        storeSession.copies
    }

    /// Held for the app's life: plug-ins and removals, and every key task.
    var keyPort: KeyPort {
        storeSession.keyPort
    }

    var reader: KeyReader {
        storeSession.reader
    }

    /// The copies' clock: Face ID deadlines, the PIN's 15 minutes and an
    /// open entry's 2 minutes in the background are all measured on it.
    var clock: Clock {
        storeSession.clock
    }

    var simulator: SimulatorControls? {
        storeSession.simulator
    }

    /// Adding, editing and deleting, and the unpushed changes; `nil` where
    /// the store cannot be written (see `AppModel+Edit.swift`).
    var edits: StoreEdits? {
        storeSession.edits
    }

    var syncing: SyncModel? {
        storeSession.syncing
    }

    // MARK: Lists

    func listing(_ folder: String) -> Result<FolderListing, Error> {
        Result { try store.folder(path: folder) }
    }

    func search(_ query: String) -> [SearchHit] {
        store.search(query: query, limit: 100)
    }

    func liveCopy(_ path: String) -> LiveCopy? {
        liveCopies.first { $0.entry == path }
    }

    /// Recently opened entries whose Face ID time has ended.
    var recentNeedingKey: [EntryRef] {
        recentPaths.filter { liveCopy($0) == nil }.prefix(5).map(EntryRef.init(path:))
    }

    /// What a row should show before it is tapped.
    func rowAccess(_ path: String) -> EntryRow.Access {
        if liveCopy(path) != nil { return .faceID }
        if case .notForThisKey = accessByPath[path] { return .anotherKey }
        return .needsKey
    }

    func refreshCopies() {
        guard let copies else {
            liveCopies = []
            return
        }
        do {
            liveCopies = try copies.list()
            copiesProblem = nil
        } catch {
            liveCopies = []
            copiesProblem = describe(error)
        }
        closeEntriesWhoseCopyEnded()
    }

    private func sweepCopies() {
        do {
            try copies?.sweep()
        } catch {
            copiesProblem = describe(error)
        }
        refreshCopies()
    }

    // MARK: Plugging the key in and out

    /// Why the key could not be read the last time paw tried, as the
    /// sheet's Plug in stage says it. `nil` once a read works.
    var attachProblem: KeyErrorWords? {
        reader.problem.map(Self.readProblemWords)
    }

    /// "paw couldn't read your key", then why and what to do.
    static func readProblemWords(_ problem: KeyReadProblem) -> KeyErrorWords {
        let error: Error
        switch problem {
        case .unsupported:
            return KeyErrorWords(
                problem: "This iPhone can't use USB keys",
                nextStep: "paw needs an iPhone with USB-C and smart card support.")
        case .failed(let failure):
            error = failure
        }
        let why: String
        switch error as? KeyError {
        case .KeyRemoved?:
            why = "It stopped answering."
        case .Reader(let detail)?:
            why = "The key said: \(detail)."
        case let error?:
            why = KeyErrorWords(error, entry: "this password").problem + "."
        case nil:
            if case .Failed(let message)? = error as? ConnectionError {
                why = message
            } else {
                why = describe(error)
            }
        }
        return KeyErrorWords(
            problem: "paw couldn't read your key",
            nextStep: "\(why) Try again, or unplug the key and plug it back in.")
    }

    /// A key was read: asks the core which entries it can open. Only the
    /// latest read's answer is kept, should an earlier key's come later.
    private func learnAccess(_ session: KeySession) {
        accessAttempt += 1
        let attempt = accessAttempt
        let store = store
        let paths = allEntryPaths()
        DispatchQueue.global(qos: .userInitiated).async {
            var access: [String: Access] = [:]
            if let fingerprint = session.info().decryptionFingerprint {
                for path in paths {
                    access[path] = store.access(entry: path, cardFingerprint: fingerprint)
                }
            }
            let found = Handoff(value: access)
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    guard let self, attempt == self.accessAttempt, !found.value.isEmpty else { return }
                    self.accessByPath = found.value
                }
            }
        }
    }

    private func allEntryPaths() -> [String] {
        var paths: [String] = []
        var folders = [home]
        while let listing = folders.popLast() {
            paths += listing.entries.map(\.path)
            for folder in listing.folders {
                if let inner = try? store.folder(path: folder.path) {
                    folders.append(inner)
                }
            }
        }
        return paths
    }

    // MARK: Opening

    /// A tap on an entry row.
    func open(_ entry: EntryRef) {
        notice = nil
        if usesSplitView {
            // Already open on the right: nothing to do.
            if splitShownEntry?.ref.path == entry.path { return }
            closeSplitEntryForNextOpen()
        }
        if liveCopy(entry.path) != nil {
            openWithFaceId(entry)
        } else {
            openWithKey(entry)
        }
    }

    /// Opens with the key: says so first if the plugged-in key cannot open
    /// it, otherwise shows the key sheet at the first stage still needed.
    func openWithKey(_ entry: EntryRef) {
        notice = nil
        guard keyRequest == nil else { return }
        closeSplitEntryForNextOpen()
        if keyPort.session() != nil, case .notForThisKey = accessByPath[entry.path] {
            show(.anotherKey(entry.path))
            return
        }
        pendingRoute = nil
        keyRequest = KeyRequest(entry: entry)
        let store = store
        let copies = copies
        let port = keyPort
        keyTask.start { listener in
            port.openEntry(
                store: store, entry: entry.path, copies: copies, ttlSecs: FaceIdOpener.windowSecs, listener: listener)
        }
    }

    /// Cancel, or the sheet swiped away.
    func cancelKeyTask() {
        keyTask.cancel()
        keyRequest = nil
    }

    /// The sheet went away, by a button, a swipe or by itself after Done.
    func keySheetDismissed() {
        if let route = pendingRoute {
            pendingRoute = nil
            show(route)
        }
    }

    /// The core found, before any PIN or touch, that this key cannot open
    /// the entry: say which keys can.
    private func showAnotherKey(_ entry: EntryRef) {
        // The key may have been plugged in after the list was worked out.
        if let fingerprint = keyPort.session()?.info().decryptionFingerprint {
            accessByPath[entry.path] = store.access(entry: entry.path, cardFingerprint: fingerprint)
        }
        show(.anotherKey(entry.path))
    }

    /// "This key: serial 12 345 678, ends 77B0 9F3A. ..."
    private static func mismatchWords(_ mismatch: KeyMismatch) -> String {
        func describeKey(_ key: KeyIdentity) -> String {
            var words = "serial \(spokenSerial(key.ident))"
            if let fingerprint = key.decryptionFingerprint {
                words += ", ending \(fingerprintEnding(fingerprint))"
            }
            return words
        }
        return "This key: \(describeKey(mismatch.presented)). The store's key: \(describeKey(mismatch.enrolled)). "
            + "Plug that one in and try again. No PIN try was used."
    }

    /// Opens from the entry's Face ID copy, with no key sheet. Falls back
    /// to the key when the copy has ended.
    private func openWithFaceId(_ entry: EntryRef) {
        guard let copies else { return }
        faceId.open(
            entry, store: store, copies: copies, reason: "Open \(entry.name)",
            answered: { [weak self] in self?.refreshCopies() }
        ) { [weak self] result in
            self?.faceIdFinished(entry, result: result)
        }
    }

    private func faceIdFinished(_ entry: EntryRef, result: Result<FaceIdOpen, Error>) {
        switch result {
        case .success(.opened(let opened)):
            noteOpened(opened, path: entry.path)
            let session = OpenSession(
                ref: entry, entry: opened, view: opened.view(), hadCopy: liveCopy(entry.path) != nil)
            sessions[session.id] = session
            show(.entry(session.id))
        case .success(.needsKey):
            openWithKey(entry)
        case .failure(let error):
            notice = Self.faceIdNotice(error, entry: entry)
        }
    }

    private static func faceIdNotice(_ error: Error, entry: EntryRef) -> Notice {
        switch error as? PawError {
        case .FaceIdCancelled:
            return Notice(
                problem: "Face ID was cancelled",
                nextStep: "Tap \(entry.name) to try again.", keyEntry: entry)
        case .FaceIdNotRecognised:
            return Notice(
                problem: "Face ID didn't recognise you",
                nextStep: "Tap \(entry.name) to try again.", keyEntry: entry)
        case .FaceIdLockedOut:
            return Notice(
                problem: "Face ID is off until you enter your iPhone passcode",
                nextStep: "Lock and unlock your iPhone, then try again.", keyEntry: entry)
        case .FaceIdNotSetUp:
            return Notice(
                problem: "Face ID isn't set up on this iPhone",
                nextStep: "Open \(entry.name) with your key.", keyEntry: entry)
        case .FaceIdChanged:
            return Notice(
                problem: "Face ID changed, so its saved copies were removed",
                nextStep: "Open \(entry.name) with your key.", keyEntry: entry)
        default:
            return Notice(problem: describe(error), nextStep: "Open \(entry.name) with your key.", keyEntry: entry)
        }
    }

    private func noteOpened(_ opened: OpenedEntry, path: String) {
        recentPaths.removeAll { $0 == path }
        recentPaths.insert(path, at: 0)
        storeSession.live.opened.send(EntryOpened(entry: opened, path: path))
    }

    func session(_ id: UUID) -> OpenSession? {
        sessions[id]
    }

    /// A save changed an open entry: the screen shows `entry`, the saved
    /// text, under the same id, and the text it showed is wiped.
    func replaceSession(_ id: UUID, ref: EntryRef, entry: OpenedEntry) {
        let hadCopy = sessions[id]?.hadCopy ?? false
        sessions[id]?.entry.close()
        sessions[id] = OpenSession(id: id, ref: ref, entry: entry, view: entry.view(), hadCopy: hadCopy)
    }

    /// Closes an open entry on screen: its text is wiped, and its route
    /// shows why, with one tap to open it again.
    func closeSession(_ id: UUID, because reason: ClosedEntry.Reason) {
        guard let session = sessions.removeValue(forKey: id) else { return }
        session.entry.close()
        closedEntries[id] = ClosedEntry(ref: session.ref, reason: reason)
    }

    func closeAllSessions(because reason: ClosedEntry.Reason) {
        for id in Array(sessions.keys) {
            closeSession(id, because: reason)
        }
    }

    /// The store on disk changed (a save, a delete, an undo, a sync):
    /// lists and search read it again. Recent entries follow a rename and
    /// drop entries that are gone. A removal locks first.
    private func storeChanged(_ changed: StoreChanged) {
        let renamed: StoreChange.Renamed?
        switch changed.change {
        case .wrote(_, let rename):
            renamed = rename
        case .synced, .reencrypted:
            renamed = nil
        case .recipientsAccepted:
            return
        case .removed:
            lockNow()
            return
        }
        if let problem = changed.readProblem {
            notice = Notice(problem: "Couldn't read the list again", nextStep: describe(problem))
            return
        }
        let store = changed.store
        home = store.home()
        entryCount = Int(store.entryCount())
        let exists = Set(allEntryPaths())
        recentPaths = recentPaths
            .map { $0 == renamed?.from ? (renamed?.to ?? $0) : $0 }
            .filter { exists.contains($0) }
        // A copy of an entry that is gone (a discarded add, a rename, a
        // delete) would otherwise sit on Home until its time ran out.
        if let copies {
            for copy in liveCopies where !exists.contains(copy.entry) {
                try? copies.forgetEntry(entry: copy.entry)
            }
        }
        refreshCopies()
    }

    // MARK: Navigation

    /// The one way the stack changes: by a screen's back, a column, or a
    /// feature asking. Every opened entry that leaves it is closed.
    func setPath(_ routes: [Route]) {
        path = routes
        pathChanged()
    }

    func show(_ route: Route) {
        setPath(path + [route])
    }

    func leave(_ route: Route) {
        setPath(path.filter { $0 != route })
    }

    /// What a feature asked the screens for.
    private func asked(_ request: ScreenRequest) {
        switch request {
        case .show(let route): show(route)
        case .leave(let route): leave(route)
        case .toast(let words): toast = words
        case .settings: showsSettings = true
        }
    }

    /// The key sheet, Settings, an entry's large type, or a feature's
    /// sheet is up.
    var sheetIsUp: Bool {
        keyRequest != nil || showsSettings || largeTypeUp || storeSession.sheetIsUp
    }

    /// Closes every opened entry that is no longer on the navigation stack.
    private func pathChanged() {
        if usesSplitView {
            // Arrives as appended to the stack; the columns want it in order.
            let columns = SplitPath(path).path
            if columns != path { path = columns }
        }
        let shown = Set(path.compactMap { route -> UUID? in
            if case .entry(let id) = route { return id }
            return nil
        })
        for (id, session) in sessions where !shown.contains(id) && pendingRoute != .entry(id) {
            session.entry.close()
            sessions[id] = nil
        }
        for id in Array(closedEntries.keys) where !shown.contains(id) {
            closedEntries[id] = nil
        }
    }

    // MARK: Lock now and the app's life

    /// The panic control: ends every Face ID copy, forgets the PIN, closes
    /// open entries and clears a password paw put on the clipboard.
    ///
    /// The core guarantees the rest: `forgetPin` drops the PIN at once and
    /// de-verifies the card on its own thread (no card traffic here), stops
    /// a PIN check already under way from holding the PIN again, and a
    /// cancelled task keeps no copy. A task past the point where cancel
    /// works ends the copies again when it finishes (`letGoTaskEnded`).
    func lockNow() {
        let ended = liveCopies.count
        cancelKeyTask()
        keyPort.session()?.forgetPin()
        do {
            try copies?.lockNow()
        } catch {
            copiesProblem = describe(error)
        }
        keyTask.generation += 1
        // A Face ID open that started before this finishes into a locked
        // app: it leaves nothing behind.
        faceId.forget()
        pendingRoute = nil
        setPath([])
        Clipboard.clearIfOurs()
        refreshCopies()
        toast = ended == 1 ? "Locked. 1 password needs your key again." : "Locked. \(ended) passwords need your key again."
    }

    /// The core forgets the PIN after 15 minutes in the background, on the
    /// clock given to the key port. An open entry closes after 2 minutes
    /// away, on the same clock (design 2.5). Coming back also reads a key
    /// whose earlier read failed.
    func scenePhaseChanged(_ phase: ScenePhase) {
        switch phase {
        case .background:
            backgroundedAtMs = clock.monotonicMs()
            keyPort.session()?.noteBackgrounded()
            simulator?.timePassesInBackground()
        case .active:
            if let away = backgroundedAtMs {
                backgroundedAtMs = nil
                if clock.monotonicMs() - away >= Self.openEntryBackgroundLimitMs {
                    closeAllSessions(because: .away)
                }
            }
            keyPort.session()?.noteForegrounded()
            storeSession.cameForward()
            Clipboard.clearIfExpired()
            sweepCopies()
            reader.readIfNeeded()
        default:
            break
        }
    }

    /// The simulated clock moved: copies may have ended.
    func advanceSimulatedClock(hours: UInt64) {
        simulator?.advanceClock(hours * 3_600_000)
        Clipboard.clearIfExpired()
        refreshCopies()
    }
}

extension AppModel: KeyTaskOwner {
    func keyWords(_ error: KeyError, pinAskedAgain: Bool) -> KeyErrorWords {
        let name = keyRequest?.entry.name ?? ""
        return pinAskedAgain ? KeyErrorWords(pinAskedAgain: error, entry: name) : KeyErrorWords(error, entry: name)
    }

    func workingStage(_ progress: TaskProgress?) -> KeySheetStage {
        .working(
            message: "Opening \(keyRequest?.entry.name ?? "")\u{2026}",
            progress: progress.map { "\($0.done) of \($0.total)" })
    }

    func attachWords(_ problem: KeyReadProblem) -> KeyErrorWords {
        Self.readProblemWords(problem)
    }

    func keyTaskEnded(_ end: KeyTaskEnd) -> KeySheetStage? {
        guard var request = keyRequest else {
            letGoTaskEnded(end)
            return nil
        }
        switch end.result {
        case .done(let outcome):
            opened(outcome, entry: request.entry)
            return .done
        case .failed(.DifferentKey(let mismatch)):
            // Not this entry's problem: the key is not the store's.
            request.differentKey = true
            keyRequest = request
            return .failed(
                problem: "This isn't the key this store was set up with",
                nextStep: Self.mismatchWords(mismatch))
        case .failed(.NotForThisKey):
            keyRequest = nil
            showAnotherKey(request.entry)
            return nil
        // An open never syncs or sets up.
        case .cancelled, .syncFailed, .setupFailed:
            keyRequest = nil
            return nil
        case .failed:
            return standardStage(end.result)
        }
    }

    func doneShown() {
        keyRequest = nil
    }

    /// A task the sheet let go of. Cancel stops it before any copy is kept,
    /// except in the last milliseconds of an open, after the card has
    /// answered. Such a task still finishes with the entry open and its
    /// copy kept: close the entry and remove that one copy. If Lock now
    /// came since the task started, end every copy again instead, which
    /// also deletes the seal key the late copy may have made.
    func letGoTaskEnded(_ end: KeyTaskEnd) {
        guard case .done(.opened(let entry, let faceIdUntilMs, _)) = end.result else { return }
        entry.close()
        guard faceIdUntilMs != nil, let copies, case .openEntry(let path) = end.purpose else { return }
        do {
            if end.generation < keyTask.generation {
                try copies.lockNow()
            } else {
                try copies.forgetEntry(entry: path)
            }
        } catch {
            copiesProblem = describe(error)
        }
        refreshCopies()
    }

    /// The entry is open: it is pushed once the sheet has gone.
    private func opened(_ outcome: KeyOutcome, entry: EntryRef) {
        guard case .opened(let opened, let faceIdUntilMs, _) = outcome else { return }
        noteOpened(opened, path: entry.path)
        refreshCopies()
        let session = OpenSession(ref: entry, entry: opened, view: opened.view(), hadCopy: faceIdUntilMs != nil)
        sessions[session.id] = session
        pendingRoute = .entry(session.id)
    }
}

/// Hooks into the simulated hardware, for the debug controls.
struct SimulatorControls {
    let advanceClock: (UInt64) -> Void
    /// Plugs the simulated key back in.
    let plugIn: () -> Void
    /// The simulated key is not plugged in at launch.
    let startsUnplugged: Bool
    /// Time that passes on the simulated clock each time paw goes to the
    /// background, so a test can stay away for 15 minutes in a second.
    let awayMs: UInt64

    func timePassesInBackground() {
        if awayMs > 0 {
            advanceClock(awayMs)
        }
    }
}

extension EntryRef {
    /// An entry from its full path, e.g. `web/github.com`.
    init(path: String) {
        if let slash = path.lastIndex(of: "/") {
            self.init(
                path: path, name: String(path[path.index(after: slash)...]),
                folder: String(path[..<slash]))
        } else {
            self.init(path: path, name: path, folder: "")
        }
    }

    /// The top-level folder, which picks the tint; `nil` at the top.
    var topFolder: String? {
        folder.isEmpty ? nil : folder.split(separator: "/").first.map(String.init)
    }
}

extension Recipient {
    /// "key ID 1234 5678 9ABC DEF0", for a sentence.
    var spoken: String {
        switch self {
        case .keyId: return "key ID \(grouped)"
        case .fingerprint: return "key \(grouped)"
        case .hidden: return "a hidden recipient"
        }
    }

    /// The hex alone, in groups of four so it can be read aloud.
    var grouped: String {
        let hex: String
        switch self {
        case .keyId(let id): hex = id
        case .fingerprint(let fingerprint): hex = fingerprint
        case .hidden: return "Hidden"
        }
        return stride(from: 0, to: hex.count, by: 4).map { offset -> String in
            let start = hex.index(hex.startIndex, offsetBy: offset)
            let end = hex.index(start, offsetBy: 4, limitedBy: hex.endIndex) ?? hex.endIndex
            return String(hex[start..<end])
        }.joined(separator: " ")
    }
}

/// Times as the screens say them: "6:40 PM".
func clockTime(ms: Int64) -> String {
    Date(timeIntervalSince1970: TimeInterval(ms) / 1000).formatted(date: .omitted, time: .shortened)
}
