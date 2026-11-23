import Foundation
import Observation
import SwiftUI
#if !targetEnvironment(simulator)
import CryptoTokenKit
#endif

/// Where the navigation stack can go.
enum Route: Hashable {
    case folder(String)
    /// An opened entry, by the id of its `OpenSession`.
    case entry(UUID)
    /// An entry the plugged-in key cannot open, explained.
    case anotherKey(String)
    /// Changes saved on this iPhone and not sent yet.
    case unpushed
}

/// An entry that is open on screen. The plaintext stays in Rust, behind
/// `entry`; `close()` wipes it when the screen goes away.
struct OpenSession: Identifiable {
    /// Kept when a save replaces the entry's text, so the screen stays.
    var id = UUID()
    let ref: EntryRef
    let entry: OpenedEntry
    let view: EntryView
}

/// The key sheet while it is up. `nil` in `AppModel.keyRequest` means no
/// sheet. The sheet stays up across "Try again", which starts a new core
/// task: `taskID` names the one the sheet follows now.
struct KeyRequest: Identifiable {
    let id = UUID()
    let entry: EntryRef
    var taskID: UUID
    var stage: KeySheetStage
}

/// A problem shown inline at the top of the list it happened in.
struct Notice: Equatable {
    let problem: String
    let nextStep: String
    /// When set, the notice offers to open this entry with the key instead.
    var keyEntry: EntryRef? = nil
}

/// Wraps a value that crosses from a card or Face ID thread to the main actor once.
private struct Handoff<Value>: @unchecked Sendable {
    let value: Value
}

/// Hands a core task's stage changes to the main queue. The core calls it
/// on its own threads, in order, and it must not block: it only queues.
private final class SheetListener: KeyTaskListener, @unchecked Sendable {
    private let changed: @Sendable () -> Void

    init(changed: @escaping @Sendable () -> Void) {
        self.changed = changed
    }

    func stateChanged(state: KeyTaskState) {
        // The state itself is read again on the main queue, so the sheet
        // always shows the latest one and never steps back.
        DispatchQueue.main.async(execute: changed)
    }
}

/// The one model of the app: owns the store, the Face ID copies and the key
/// port, and gives views plain values. The PIN lives in the core's key
/// session, never here. Card and Face ID calls block for seconds, so they
/// run off the main thread and their results are published here.
@MainActor @Observable
final class AppModel {
    typealias Connect = @Sendable () throws -> CardConnection

    /// How long an entry opened with the key stays available to Face ID.
    static let faceIdWindowSecs: UInt32 = 8 * 60 * 60

    /// Replaced after each save, delete or undo (`storeChanged`).
    private(set) var store: PassStore
    /// `nil` where Face ID copies cannot be kept: on a device until the
    /// Secure Enclave keystore exists. Every open then uses the key.
    let copies: CopyCache?
    /// Held for the app's life: plug-ins and removals, and every key task.
    let keyPort: KeyPort
    private let connect: Connect
    /// Debug-only controls for the simulated hardware, `nil` on a device.
    let simulator: SimulatorControls?

    var path: [Route] = []
    private(set) var home: FolderListing
    private(set) var entryCount: Int
    /// Adding, editing and deleting, and the unpushed changes; `nil` where
    /// the store cannot be written (see `AppModel+Edit.swift`).
    var edits: StoreEdits?
    private(set) var liveCopies: [LiveCopy] = []
    /// Entry paths opened in this run, most recent first.
    private(set) var recentPaths: [String] = []
    /// What the core says about each entry for the last key plugged in.
    private(set) var accessByPath: [String: Access] = [:]
    var keyRequest: KeyRequest?
    /// What is typed in the key sheet's PIN field. Handed to the core and
    /// cleared at once on Continue; cleared too whenever the sheet moves on.
    var pinEntry = ""
    private(set) var sessions: [UUID: OpenSession] = [:]
    var notice: Notice?
    /// A problem with the Face ID copies themselves, shown on Home.
    private(set) var copiesProblem: String?
    var toast: String?
    var showsSettings = false
    /// The entry Face ID is being asked for, to ignore a second tap.
    private(set) var faceIdOpening: String?

    /// Every core task whose end has not been seen yet: the one the sheet
    /// follows, and any it let go of (Cancel, Lock now) that may still
    /// finish. Each finishes exactly once here.
    @ObservationIgnored private var tasks: [UUID: KeyTask] = [:]
    /// Bumped by Lock now. A Face ID open that started before the latest
    /// bump finished into a locked app: it must leave nothing behind.
    @ObservationIgnored private var lockGeneration = 0
    @ObservationIgnored private var pendingRoute: Route?
    @ObservationIgnored private var plugInCount = 0
    #if !targetEnvironment(simulator)
    @ObservationIgnored private var slotWatch: NSKeyValueObservation?
    #endif
    private let cardQueue = DispatchQueue(label: "paw-card", qos: .userInitiated)
    private let faceIdQueue = DispatchQueue(label: "paw-bio", qos: .userInitiated)

    /// `clock` is the one the copies use: the PIN's 15 minutes in the
    /// background are measured on it.
    /// `keyPort` is setup's when Home follows setup, so the PIN typed there
    /// is still held.
    init(store: PassStore, copies: CopyCache?, clock: Clock, connect: @escaping Connect, simulator: SimulatorControls?,
         keyPort: KeyPort? = nil) {
        self.store = store
        self.copies = copies
        self.connect = connect
        self.simulator = simulator
        self.keyPort = keyPort ?? KeyPort(clock: clock)
        home = store.home()
        entryCount = Int(store.entryCount())
        sweepCopies()
        #if targetEnvironment(simulator)
        if simulator?.startsUnplugged != true {
            plugIn(reusingSession: true)
        }
        #else
        watchReaders()
        #endif
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

    /// A key is there: the core reads it (no PIN, no touch) and makes its
    /// session, and a task waiting at Plug in goes on by itself. Then asks
    /// the core which entries this key can open.
    func plugIn() {
        plugIn(reusingSession: false)
    }

    /// `reusingSession`: a key session already open (setup's) is kept,
    /// with the PIN it holds, instead of reading the key again.
    private func plugIn(reusingSession: Bool) {
        plugInCount += 1
        let attempt = plugInCount
        let connect = connect
        let port = keyPort
        let store = store
        let paths = allEntryPaths()
        cardQueue.async {
            var access: [String: Access] = [:]
            let held = reusingSession ? port.session() : nil
            if let session = held ?? (try? port.pluggedIn(connection: connect())),
               let fingerprint = session.info().decryptionFingerprint {
                for path in paths {
                    access[path] = store.access(entry: path, cardFingerprint: fingerprint)
                }
            }
            // A key that cannot be read leaves any task at Plug in, which
            // is still true: no key paw can use is plugged in.
            let found = Handoff(value: access)
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    guard let self, attempt == self.plugInCount, !found.value.isEmpty else { return }
                    self.accessByPath = found.value
                }
            }
        }
    }

    /// The key went away: its session ends and the core drops the PIN. A
    /// task using it fails with "Your key was unplugged".
    func unplugged() {
        plugInCount += 1
        keyPort.unplugged()
    }

    #if targetEnvironment(simulator)
    /// The debug control on the Plug in stage.
    func simulatedPlugIn() {
        simulator?.plugIn()
        plugIn()
    }
    #else
    /// Follows the system's list of smart card readers: a YubiKey on the
    /// USB-C port shows up as one. Minimal, and unverified on hardware: a
    /// reader appearing plugs in, the list emptying unplugs. A key pulled
    /// out mid-command is also caught by the core as a reader failure.
    private func watchReaders() {
        guard let manager = TKSmartCardSlotManager.default else { return }
        // Called on whatever thread the system changes the list on.
        slotWatch = manager.observe(\.slotNames, options: [.initial, .new]) { [weak self] manager, _ in
            let present = !manager.slotNames.isEmpty
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.readersChanged(present: present)
                }
            }
        }
    }

    private func readersChanged(present: Bool) {
        if !present {
            unplugged()
        } else if keyPort.session() == nil {
            plugIn()
        }
    }
    #endif

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
        if keyPort.session() != nil, case .notForThisKey = accessByPath[entry.path] {
            path.append(.anotherKey(entry.path))
            return
        }
        pendingRoute = nil
        let taskID = startTask(entry)
        keyRequest = KeyRequest(entry: entry, taskID: taskID, stage: .busy)
        taskChanged(taskID)
    }

    /// Starts a core task that opens `entry` and keeps its Face ID copy.
    private func startTask(_ entry: EntryRef) -> UUID {
        pinEntry = ""
        let id = UUID()
        let listener = SheetListener { [weak self] in
            MainActor.assumeIsolated {
                self?.taskChanged(id)
            }
        }
        tasks[id] = keyPort.openEntry(
            store: store, entry: entry.path, copies: copies, ttlSecs: Self.faceIdWindowSecs, listener: listener)
        return id
    }

    /// Continue on the PIN stage. The PIN goes straight to the core, which
    /// refuses one that is not 6 to 127 characters without using a try.
    func continueWithPin() {
        guard let request = keyRequest, let task = tasks[request.taskID], !pinEntry.isEmpty else { return }
        switch request.stage {
        case .pin, .wrongPin, .pinAgain: break
        default: return
        }
        task.supplyPin(pin: Data(pinEntry.utf8))
        pinEntry = ""
    }

    /// "Try again" on the failed stage: a new task, which opens at the
    /// first stage still needed (Touch if the PIN is still held).
    func retry() {
        guard var request = keyRequest else { return }
        letGo(request.taskID)
        request.taskID = startTask(request.entry)
        request.stage = .busy
        keyRequest = request
        taskChanged(request.taskID)
    }

    /// Cancel always works. A card command already waiting for a touch
    /// cannot be stopped; the core throws its answer away.
    func cancelKeyTask() {
        if let request = keyRequest {
            letGo(request.taskID)
        }
        keyRequest = nil
        pinEntry = ""
    }

    /// The sheet went away, by a button, a swipe or by itself after Done.
    func keySheetDismissed() {
        if let route = pendingRoute {
            pendingRoute = nil
            path.append(route)
        }
    }

    /// A task changed stage (or was just started): show it if the sheet
    /// follows it, settle it if the sheet let go of it.
    private func taskChanged(_ id: UUID) {
        guard let task = tasks[id] else { return }
        let state = task.state()
        guard var request = keyRequest, request.taskID == id else {
            settle(id, state)
            return
        }
        if state.isFinal {
            tasks[id] = nil
        }
        switch state {
        case .plugIn:
            request.stage = .plugIn
        case .pin(let triesLeft, let forgotten, let lastError):
            let tries = Int(triesLeft ?? 3)
            switch lastError {
            case nil:
                request.stage = .pin(triesLeft: tries, forgotten: forgotten)
            case .WrongPin(let left)?:
                request.stage = .wrongPin(triesLeft: Int(left))
            case let error?:
                let words = KeyErrorWords(error, entry: request.entry.name)
                request.stage = .pinAgain(triesLeft: tries, problem: words.problem, nextStep: words.nextStep)
            }
        case .busy:
            request.stage = .busy
        case .touch:
            request.stage = .touch
        case .working(let progress):
            request.stage = .working(
                message: "Opening \(request.entry.name)\u{2026}",
                progress: progress.map { "\($0.done) of \($0.total)" })
        case .done(let outcome):
            finished(outcome, request: &request)
        case .failed(.NotForThisKey):
            keyRequest = nil
            showAnotherKey(request.entry)
            return
        case .failed(.PinBlocked):
            request.stage = .locked
        case .failed(.Cancelled), .cancelled:
            keyRequest = nil
            return
        case .failed(let error):
            let words = KeyErrorWords(error, entry: request.entry.name)
            request.stage = .failed(problem: words.problem, nextStep: words.nextStep)
        }
        if !Self.asksForPin(request.stage) {
            pinEntry = ""
        }
        keyRequest = request
    }

    private static func asksForPin(_ stage: KeySheetStage) -> Bool {
        switch stage {
        case .pin, .wrongPin, .pinAgain: true
        default: false
        }
    }

    private func finished(_ outcome: KeyOutcome, request: inout KeyRequest) {
        guard case .opened(let opened, _) = outcome else {
            request.stage = .done
            return
        }
        noteOpened(request.entry.path)
        refreshCopies()
        let session = OpenSession(ref: request.entry, entry: opened, view: opened.view())
        sessions[session.id] = session
        pendingRoute = .entry(session.id)
        request.stage = .done
        let sheet = request.id
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard let self, keyRequest?.id == sheet else { return }
            keyRequest = nil
        }
    }

    /// The core found, before any PIN or touch, that this key cannot open
    /// the entry: say which keys can.
    private func showAnotherKey(_ entry: EntryRef) {
        // The key may have been plugged in after the list was worked out.
        if let fingerprint = keyPort.session()?.info().decryptionFingerprint {
            accessByPath[entry.path] = store.access(entry: entry.path, cardFingerprint: fingerprint)
        }
        path.append(.anotherKey(entry.path))
    }

    /// The sheet stops following a task: it is cancelled, and settled when
    /// it ends.
    private func letGo(_ id: UUID) {
        guard let task = tasks[id] else { return }
        task.cancel()
        settle(id, task.state())
    }

    /// A task the sheet no longer follows. Cancel stops it before any copy
    /// is kept, except in the last milliseconds of an open, after the card
    /// has answered (`core/src/key/task.rs`). Such a task still finishes
    /// with the entry open and its copy kept: close the entry, and end the
    /// copies, because the core cannot remove one entry's copy (core
    /// request). Lock now relies on this too.
    private func settle(_ id: UUID, _ state: KeyTaskState) {
        guard state.isFinal else { return }
        tasks[id] = nil
        guard case .done(.opened(let entry, let faceIdUntilMs)) = state else { return }
        entry.close()
        if faceIdUntilMs != nil {
            endCopiesLeftByLateOpen()
        }
    }

    private func endCopiesLeftByLateOpen() {
        do {
            try copies?.lockNow()
        } catch {
            copiesProblem = describe(error)
        }
        refreshCopies()
    }

    /// Opens from the entry's Face ID copy, with no key sheet. Falls back
    /// to the key when the copy has ended.
    private func openWithFaceId(_ entry: EntryRef) {
        guard faceIdOpening == nil, let copies else { return }
        faceIdOpening = entry.path
        let store = store
        let started = lockGeneration
        faceIdQueue.async {
            let result = Result {
                try store.openEntryWithFaceId(copies: copies, entry: entry.path, reason: "Open \(entry.name)")
            }
            let handoff = Handoff(value: result)
            Task { @MainActor [weak self] in self?.faceIdFinished(entry, started: started, result: handoff.value) }
        }
    }

    private func faceIdFinished(_ entry: EntryRef, started: Int, result: Result<FaceIdOpen, Error>) {
        faceIdOpening = nil
        refreshCopies()
        guard started == lockGeneration else {
            // Locked while Face ID was up: nothing opens.
            if case .success(.opened(let opened)) = result { opened.close() }
            return
        }
        switch result {
        case .success(.opened(let opened)):
            noteOpened(entry.path)
            let session = OpenSession(ref: entry, entry: opened, view: opened.view())
            sessions[session.id] = session
            path.append(.entry(session.id))
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
                problem: "Face ID was cancelled, so \(entry.name) didn't open.",
                nextStep: "Tap it again to use Face ID.", keyEntry: entry)
        case .FaceIdNotRecognised:
            return Notice(
                problem: "Face ID didn't recognise you.",
                nextStep: "Tap \(entry.name) to try again.", keyEntry: entry)
        case .FaceIdLockedOut:
            return Notice(
                problem: "Face ID is turned off until you enter your iPhone passcode.",
                nextStep: "Lock and unlock your iPhone, then try again.", keyEntry: entry)
        case .FaceIdNotSetUp:
            return Notice(
                problem: "Face ID isn't set up on this iPhone, so saved copies can't be opened.",
                nextStep: "Open \(entry.name) with your key instead.", keyEntry: entry)
        case .FaceIdChanged:
            return Notice(
                problem: "Face ID changed on this iPhone, so saved copies were removed.",
                nextStep: "Open \(entry.name) with your key.", keyEntry: entry)
        default:
            return Notice(problem: describe(error), nextStep: "Open \(entry.name) with your key instead.", keyEntry: entry)
        }
    }

    private func noteOpened(_ path: String) {
        recentPaths.removeAll { $0 == path }
        recentPaths.insert(path, at: 0)
    }

    func session(_ id: UUID) -> OpenSession? {
        sessions[id]
    }

    /// A save changed an open entry: the screen shows `entry`, the saved
    /// text, under the same id, and the text it showed is wiped.
    func replaceSession(_ id: UUID, ref: EntryRef, entry: OpenedEntry) {
        sessions[id]?.entry.close()
        sessions[id] = OpenSession(id: id, ref: ref, entry: entry, view: entry.view())
    }

    /// The store on disk changed (a save, a delete, an undo): lists and
    /// search read it again. Recent entries follow a rename and drop
    /// entries that are gone.
    func storeChanged(_ store: PassStore, renamed: (from: String, to: String)? = nil) {
        self.store = store
        home = store.home()
        entryCount = Int(store.entryCount())
        let exists = Set(allEntryPaths())
        recentPaths = recentPaths
            .map { $0 == renamed?.from ? (renamed?.to ?? $0) : $0 }
            .filter { exists.contains($0) }
        refreshCopies()
    }

    /// Closes every opened entry that is no longer on the navigation stack.
    func pathChanged() {
        let shown = Set(path.compactMap { route -> UUID? in
            if case .entry(let id) = route { return id }
            return nil
        })
        for (id, session) in sessions where !shown.contains(id) && pendingRoute != .entry(id) {
            session.entry.close()
            sessions[id] = nil
        }
    }

    // MARK: Lock now and the app's life

    /// The panic control: ends every Face ID copy, forgets the PIN, closes
    /// open entries and clears a password paw put on the clipboard.
    ///
    /// The core guarantees the rest: `forgetPin` stops a PIN check already
    /// under way from holding the PIN again, and a cancelled task keeps no
    /// copy. A task past the point where cancel works ends the copies again
    /// when it finishes (`settle`).
    func lockNow() {
        let ended = liveCopies.count
        if let request = keyRequest {
            letGo(request.taskID)
        }
        keyRequest = nil
        pinEntry = ""
        keyPort.session()?.forgetPin()
        do {
            try copies?.lockNow()
        } catch {
            copiesProblem = describe(error)
        }
        lockGeneration += 1
        pendingRoute = nil
        path = []
        pathChanged()
        Clipboard.clearIfOurs()
        refreshCopies()
        toast = ended == 1 ? "Locked. 1 entry now needs your key." : "Locked. \(ended) entries now need your key."
    }

    /// The core forgets the PIN after 15 minutes in the background, on the
    /// clock given to the key port.
    func scenePhaseChanged(_ phase: ScenePhase) {
        switch phase {
        case .background:
            keyPort.session()?.noteBackgrounded()
            simulator?.timePassesInBackground()
        case .active:
            keyPort.session()?.noteForegrounded()
            Clipboard.clearIfExpired()
            sweepCopies()
        default:
            break
        }
    }

    /// The simulated clock moved: copies may have ended.
    func advanceSimulatedClock(hours: UInt64) {
        simulator?.advanceClock(hours * 3_600_000)
        refreshCopies()
    }
}

extension KeyTaskState {
    /// Done, failed or cancelled: nothing changes it any more.
    var isFinal: Bool {
        switch self {
        case .done, .failed, .cancelled: true
        default: false
        }
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
