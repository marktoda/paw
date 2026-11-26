import Foundation
import Observation
import SwiftUI

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

struct KeyRequest: Identifiable {
    /// The sheet's run: the same across Try again.
    let id: UUID
    let entry: EntryRef
}

/// A problem shown inline at the top of the list it happened in.
struct Notice: Equatable {
    let problem: String
    let nextStep: String
    /// When set, the notice offers to open this entry with the key instead.
    var keyEntry: EntryRef? = nil
}

/// A short line at the bottom of the screen: a copy, or what a feature
/// said ("Saved. Not sent yet.").
struct Toast: Equatable {
    enum Kind {
        case copied, said
    }

    let text: String
    var kind: Kind = .said
}

/// The model of the screens over a finished `StoreSession`: navigation,
/// the key sheet and open entries. Features live on the session and ask
/// the screens for what they need through its `requests`. The PIN lives in
/// the core's key session, never here. Card and Face ID calls block for
/// seconds, so they run off the main thread and their results are
/// published here.
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
    private(set) var liveCopies: [LiveCopy] = []
    /// Entry paths opened in this run, most recent first.
    private(set) var recentPaths: [String] = []
    /// What the core says about each entry for the last key plugged in.
    private(set) var accessByPath: [String: Access] = [:]
    private var keyEntry: EntryRef?
    private(set) var sessions: [UUID: OpenSession] = [:]
    /// Entries closed while on screen (2 minutes away, the iPhone locked,
    /// the Face ID time ended), by the id their route still carries.
    var closedEntries: [UUID: ClosedEntry] = [:]
    var notice: Notice?
    /// A problem with the Face ID copies themselves, shown on Home.
    private(set) var copiesProblem: String?
    var toast: Toast?
    var showsSettings = false
    /// The open entry's value in large type (a full-screen cover on iPad).
    var largeType: LargeTypeItem?
    /// The sheets up other than key sheets.
    let sheets = SheetsUp()

    @ObservationIgnored private var pendingRoute: Route?
    /// Lock now's so far, so an open can tell it came since the open began.
    @ObservationIgnored private var locks = 0
    /// Bumped by each key read: which access answer is the latest.
    @ObservationIgnored private var accessAttempt = 0
    /// Monotonic milliseconds on `clock` when paw went to the background.
    @ObservationIgnored var backgroundedAtMs: Int64?
    @ObservationIgnored var deviceLockWatch: NSObjectProtocol?

    /// `notice` is said on Home from the start.
    init(session: StoreSession, notice: Notice? = nil) {
        storeSession = session
        keyTask = session.keyTasks.make()
        faceId = FaceIdOpener()
        self.notice = notice ?? session.syncProblem.map { Notice(problem: SyncWords.off, nextStep: describe($0)) }
        session.reader.onRead = { [weak self] read in
            if case .success(let keySession) = read {
                self?.learnAccess(keySession)
            }
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

    private var copies: CopyCache? {
        storeSession.copies
    }

    private var keyPort: KeyPort {
        storeSession.keyPort
    }

    private var reader: KeyReader {
        storeSession.reader
    }

    private var clock: Clock {
        storeSession.clock
    }

    // MARK: Face ID copies

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

    /// Reads the copies again. `problem` is what the change to them just
    /// before ran into, said on Home unless the list itself can't be read.
    func refreshCopies(after problem: Error? = nil) {
        guard let copies else {
            liveCopies = []
            return
        }
        do {
            liveCopies = try copies.list()
            copiesProblem = problem.map { describe($0) }
        } catch {
            liveCopies = []
            copiesProblem = describe(error)
        }
        closeEntriesWhoseCopyEnded()
    }

    private func sweepCopies() {
        var problem: Error?
        do {
            try copies?.sweep()
        } catch {
            problem = error
        }
        refreshCopies(after: problem)
    }

    // MARK: Plugging the key in and out

    /// Why the key could not be read the last time paw tried, as the
    /// sheet's Plug in stage says it. `nil` once a read works.
    var attachProblem: KeyErrorWords? {
        reader.problem.map { KeyErrorWords($0, table: .app) }
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
        store.allEntries().map(\.path)
    }

    // MARK: Opening

    /// A tap on an entry row.
    func open(_ entry: EntryRef) {
        notice = nil
        if usesSplitView {
            if splitShownEntry?.ref.path == entry.path { return }
            closeSplitEntryForNextOpen()
        }
        if liveCopy(entry.path) != nil {
            openWithFaceId(entry)
        } else {
            openWithKey(entry)
        }
    }

    /// The key sheet while it is up.
    var keyRequest: KeyRequest? {
        guard let run = keyTask.shown, let keyEntry else { return nil }
        return KeyRequest(id: run, entry: keyEntry)
    }

    /// Opens with the key: says so first if the plugged-in key cannot open
    /// it, otherwise shows the key sheet at the first stage still needed.
    func openWithKey(_ entry: EntryRef) {
        notice = nil
        guard !keyTask.isShown else { return }
        closeSplitEntryForNextOpen()
        if keyPort.session() != nil, case .notForThisKey = accessByPath[entry.path] {
            show(.anotherKey(entry.path))
            return
        }
        pendingRoute = nil
        keyEntry = entry
        keyTask.start(openJob(entry))
    }

    /// Cancel, or the sheet swiped away.
    func cancelKeyTask() {
        keyTask.cancel()
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
            let words = FaceIdWords.notice(error, entry: entry.name, table: .app)
            notice = Notice(problem: words.problem, nextStep: words.nextStep, keyEntry: words.offersKey ? entry : nil)
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
        let exists = Set(allEntryPaths())
        recentPaths = recentPaths
            .map { $0 == renamed?.from ? (renamed?.to ?? $0) : $0 }
            .filter { exists.contains($0) }
        // A copy of an entry that is gone (a discarded add, a rename, a
        // delete) would otherwise sit on Home until its time ran out.
        var problem: Error?
        if let copies {
            for copy in liveCopies where !exists.contains(copy.entry) {
                do {
                    try copies.forgetEntry(entry: copy.entry)
                } catch {
                    problem = error
                }
            }
        }
        refreshCopies(after: problem)
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
        case .toast(let words): toast = Toast(text: words)
        case .notice(let notice): self.notice = notice
        case .settings: showsSettings = true
        case .saved(let id, let saved):
            if let opened = saved.opened {
                replaceSession(id, ref: EntryRef(path: saved.entry), entry: opened)
            } else {
                // Saved, but the screen can't show the new text: close it.
                leave(.entry(id))
            }
        case .deleted(let id):
            leaveDeleted(id)
        }
    }

    /// Back from a deleted entry to the list it was in; a folder whose
    /// last entry went is gone too.
    private func leaveDeleted(_ id: UUID) {
        var routes = path
        if routes.last == .entry(id) {
            routes.removeLast()
        }
        while case .folder(let folder)? = routes.last, (try? store.folder(path: folder)) == nil {
            routes.removeLast()
        }
        setPath(routes)
    }

    /// A sheet is up, key sheet or other: the iPad's keyboard shortcuts
    /// do nothing under it.
    var sheetIsUp: Bool {
        storeSession.keyTasks.sheetShown || sheets.any
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
    /// works ends the copies again when it finishes (`openJob`'s `letGo`).
    func lockNow() {
        let ended = liveCopies.count
        locks += 1
        keyTask.cancelAll()
        keyPort.session()?.forgetPin()
        var problem: Error?
        do {
            try copies?.lockNow()
        } catch {
            problem = error
        }
        // A Face ID open that started before this finishes into a locked
        // app: it leaves nothing behind.
        faceId.forget()
        pendingRoute = nil
        setPath([])
        storeSession.clipboard.clearIfOurs()
        refreshCopies(after: problem)
        toast = Toast(text: lockedToast(ended: ended))
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
        case .active:
            if let away = backgroundedAtMs {
                backgroundedAtMs = nil
                if clock.monotonicMs() - away >= Self.openEntryBackgroundLimitMs {
                    closeAllSessions(because: .away)
                }
            }
            keyPort.session()?.noteForegrounded()
            storeSession.cameForward()
            storeSession.clipboard.clearIfExpired()
            sweepCopies()
            reader.readIfNeeded()
        default:
            break
        }
    }

    /// Time moved on: copies may have ended, and with them open entries,
    /// and a password on the clipboard may be due to go.
    func timePassed() {
        storeSession.clipboard.clearIfExpired()
        refreshCopies()
    }
}

extension AppModel {
    /// Opening `entry` with the key: the entry is pushed once the sheet has
    /// gone. One that the key cannot open says which keys can instead.
    private func openJob(_ entry: EntryRef) -> KeyJob<Never> {
        let open = KeyOpen(entry: entry, store: store, port: keyPort, copies: copies, table: .app)
        let locksAtStart = locks
        return open.job(
            opened: { [weak self] opened in
                self?.opened(opened, entry: entry)
                return .standard
            },
            failed: { [weak self] error in
                guard case .NotForThisKey = error else { return .standard }
                self?.showAnotherKey(entry)
                return .close
            },
            lateCopyKept: { [weak self] in
                guard let self else { return }
                self.removeLateCopy(of: entry, lockedSince: self.locks != locksAtStart)
            })
    }

    /// An open the sheet let go of kept a copy. Cancel stops an open before
    /// any copy is kept, except in the last milliseconds after the card has
    /// answered: remove that one copy. If Lock now came since the open
    /// began, end every copy again instead, which also deletes the seal key
    /// the late copy may have made.
    private func removeLateCopy(of entry: EntryRef, lockedSince: Bool) {
        guard let copies else { return }
        var problem: Error?
        do {
            if lockedSince {
                try copies.lockNow()
            } else {
                try copies.forgetEntry(entry: entry.path)
            }
        } catch {
            problem = error
        }
        refreshCopies(after: problem)
    }

    /// The entry is open: it is pushed once the sheet has gone. A copy that
    /// was asked for and not kept is said on Home.
    private func opened(_ opened: KeyOpen.Opened, entry: EntryRef) {
        noteOpened(opened.entry, path: entry.path)
        refreshCopies()
        let session = OpenSession(
            ref: entry, entry: opened.entry, view: opened.entry.view(), hadCopy: opened.faceIdUntilMs != nil)
        sessions[session.id] = session
        pendingRoute = .entry(session.id)
        if let error = opened.copyFailed {
            notice = Notice(problem: "Face ID couldn't keep a copy", nextStep: describe(error))
        }
    }
}

extension EntryRef {
    /// An entry from its full path, e.g. `web/github.com`.
    init(path: String) {
        self = entryRef(path: path)
    }

    /// The top-level folder, which picks the tint; `nil` at the top.
    var topFolder: String? {
        topFolderOf(folder: folder)
    }
}
