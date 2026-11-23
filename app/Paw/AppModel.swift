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
}

/// An entry that is open on screen. The plaintext stays in Rust, behind
/// `entry`; `close()` wipes it when the screen goes away.
struct OpenSession: Identifiable {
    let id = UUID()
    let ref: EntryRef
    let entry: OpenedEntry
    let view: EntryView
}

/// The key sheet while it is up. `nil` in `AppModel.keyTask` means no sheet.
struct KeyTask: Identifiable {
    let id = UUID()
    let entry: EntryRef
    var stage: KeySheetStage
    /// The core reported that the key is waiting for a touch: the PIN was
    /// accepted by then.
    var touchRequested = false
}

/// A problem shown inline at the top of the list it happened in.
struct Notice: Equatable {
    let problem: String
    let nextStep: String
    /// When set, the notice offers to open this entry with the key instead.
    var keyEntry: EntryRef? = nil
}

/// Wraps a value that crosses from the card thread to the main actor once.
private struct Handoff<Value>: @unchecked Sendable {
    let value: Value
}

/// The one model of the app: owns the store, the Face ID copies, the key
/// connection and the held PIN, and gives views plain values. Card and
/// Face ID calls block for seconds, so they run off the main thread and
/// their results are published here.
@MainActor @Observable
final class AppModel {
    typealias Connect = @Sendable (@escaping @Sendable () -> Void) throws -> CardConnection

    /// How long an entry opened with the key stays available to Face ID.
    static let faceIdWindowSecs: UInt32 = 8 * 60 * 60
    /// The PIN is forgotten after this long with paw in the background.
    static let pinForgetAfter: TimeInterval = 15 * 60

    let store: PassStore
    let copies: CopyCache
    private let connect: Connect
    /// Debug-only controls for the simulated hardware, `nil` on a device.
    let simulator: SimulatorControls?

    var path: [Route] = []
    let home: FolderListing
    let entryCount: Int
    private(set) var liveCopies: [LiveCopy] = []
    /// Entry paths opened in this run, most recent first.
    private(set) var recentPaths: [String] = []
    /// What the core says about each entry for the plugged-in key.
    private(set) var accessByPath: [String: Access] = [:]
    private(set) var card: CardInfo?
    var keyTask: KeyTask?
    /// What is typed in the key sheet's PIN field.
    var pin = ""
    private(set) var sessions: [UUID: OpenSession] = [:]
    var notice: Notice?
    /// A problem with the Face ID copies themselves, shown on Home.
    private(set) var copiesProblem: String?
    var toast: String?
    var showsSettings = false
    /// The entry Face ID is being asked for, to ignore a second tap.
    private(set) var faceIdOpening: String?

    @ObservationIgnored private var heldPin: String?
    @ObservationIgnored private var pinForgotten = false
    /// Bumped by Lock now. A card or Face ID call that started before the
    /// latest bump finished into a locked app: it must leave nothing behind.
    @ObservationIgnored private var lockGeneration = 0
    /// Bumped by Lock now and whenever the held PIN is forgotten. A call
    /// started before the latest bump may not hold its PIN again.
    @ObservationIgnored private var pinGeneration = 0
    @ObservationIgnored private var pendingRoute: Route?
    @ObservationIgnored private var backgroundedAt: Date?
    private let cardQueue = DispatchQueue(label: "paw-card", qos: .userInitiated)
    private let faceIdQueue = DispatchQueue(label: "paw-bio", qos: .userInitiated)

    init(store: PassStore, copies: CopyCache, connect: @escaping Connect, simulator: SimulatorControls?) {
        self.store = store
        self.copies = copies
        self.connect = connect
        self.simulator = simulator
        home = store.home()
        entryCount = Int(store.entryCount())
        sweepCopies()
        readKey()
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
            try copies.sweep()
        } catch {
            copiesProblem = describe(error)
        }
        refreshCopies()
    }

    // MARK: The key

    /// Reads the plugged-in key (no PIN, no touch), then asks the core
    /// which entries it can open.
    private func readKey(then next: (@MainActor @Sendable () -> Void)? = nil) {
        let connect = connect
        let store = store
        let paths = allEntryPaths()
        cardQueue.async {
            let read = Result { try readCard(connection: connect {}) }
            var access: [String: Access] = [:]
            if case .success(let info) = read, let fingerprint = info.decryptionFingerprint {
                for path in paths {
                    access[path] = store.access(entry: path, cardFingerprint: fingerprint)
                }
            }
            let result = Handoff(value: (read, access))
            Task { @MainActor [weak self] in
                guard let self else { return }
                if case .success(let info) = result.value.0 {
                    card = info
                    accessByPath = result.value.1
                }
                next?()
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
        if liveCopy(entry.path) != nil {
            openWithFaceId(entry)
        } else {
            openWithKey(entry)
        }
    }

    /// Opens with the key: says so first if this key cannot open it, then
    /// shows the key sheet at the first stage still needed.
    func openWithKey(_ entry: EntryRef) {
        notice = nil
        guard keyTask == nil else { return }
        guard let fingerprint = card?.decryptionFingerprint else {
            // No key read yet: read it now and decide then.
            readKey { [weak self] in
                guard let self else { return }
                if card?.decryptionFingerprint != nil {
                    openWithKey(entry)
                } else {
                    presentKeySheet(entry)
                }
            }
            return
        }
        let access = accessByPath[entry.path] ?? store.access(entry: entry.path, cardFingerprint: fingerprint)
        accessByPath[entry.path] = access
        if case .notForThisKey = access {
            path.append(.anotherKey(entry.path))
            return
        }
        presentKeySheet(entry)
    }

    private func presentKeySheet(_ entry: EntryRef) {
        keyTask = KeyTask(entry: entry, stage: .busy)
        start()
    }

    /// Puts the sheet at the first stage still needed, and starts the key
    /// when the PIN is already held.
    private func start() {
        guard var task = keyTask else { return }
        pin = ""
        task.touchRequested = false
        if let card {
            if card.pinTriesLeft == 0 {
                task.stage = .locked
            } else if heldPin != nil {
                task.stage = .busy
            } else {
                task.stage = .pin(triesLeft: Int(card.pinTriesLeft), forgotten: pinForgotten)
            }
        } else {
            task.stage = .plugIn
        }
        keyTask = task
        if task.stage == .busy, let heldPin {
            run(heldPin)
        }
    }

    /// Continue on the PIN stage.
    func continueWithPin() {
        guard var task = keyTask, pin.count >= 6 else { return }
        switch task.stage {
        case .pin, .wrongPin: break
        default: return
        }
        task.stage = .busy
        keyTask = task
        run(pin)
    }

    /// "Try again" on the failed stage.
    func retry() {
        start()
    }

    /// Cancel always works. A card call already under way cannot be
    /// stopped; its result is thrown away when it comes.
    func cancelKeyTask() {
        keyTask = nil
        pin = ""
    }

    /// The sheet went away, by a button, a swipe or by itself after Done.
    func keySheetDismissed() {
        if let route = pendingRoute {
            pendingRoute = nil
            path.append(route)
        }
    }

    private func run(_ pin: String) {
        guard let task = keyTask else { return }
        let taskID = task.id
        let entry = task.entry
        let connect = connect
        let store = store
        let copies = copies
        let ttl = Self.faceIdWindowSecs
        let started = Generations(lock: lockGeneration, pin: pinGeneration)
        cardQueue.async {
            let result = Result { () throws -> KeyOpen in
                let connection = try connect {
                    Task { @MainActor [weak self] in self?.touchNeeded(taskID, pin: pin, started: started) }
                }
                return try store.openEntryKeepingCopy(
                    connection: connection, pin: pin, entry: entry.path, copies: copies, ttlSecs: ttl)
            }
            let handoff = Handoff(value: result)
            Task { @MainActor [weak self] in
                self?.keyOpenFinished(taskID, entry: entry, pin: pin, started: started, result: handoff.value)
            }
        }
    }

    /// The PIN may be held again only if nothing forgot it since the call
    /// started. A plain Cancel does not forget it: the person typed the
    /// right PIN and the key stayed plugged in, so the next open in this
    /// run skips the PIN stage. Lock now forgets it, and a call that was
    /// under way during Lock now never brings it back.
    private func holdPin(_ pin: String, started: Generations) {
        guard started.pin == pinGeneration else { return }
        heldPin = pin
        pinForgotten = false
    }

    private func touchNeeded(_ taskID: UUID, pin: String, started: Generations) {
        // The key only waits for a touch once it has accepted the PIN.
        holdPin(pin, started: started)
        guard started.lock == lockGeneration, var task = keyTask, task.id == taskID else { return }
        task.stage = .touch
        task.touchRequested = true
        keyTask = task
        self.pin = ""
    }

    private func keyOpenFinished(
        _ taskID: UUID, entry: EntryRef, pin: String, started: Generations, result: Result<KeyOpen, Error>
    ) {
        guard started.lock == lockGeneration else {
            // Lock now ran while the key was working. The core has already
            // kept a Face ID copy for this entry: end every copy again, close
            // the entry, and leave the PIN and the recents alone.
            if case .success(let opened) = result {
                opened.entry.close()
                endCopiesLeftByLateOpen()
            }
            return
        }
        let current = keyTask?.id == taskID ? keyTask : nil
        switch result {
        case .success(let opened):
            holdPin(pin, started: started)
            guard var task = current else {
                // Cancelled: nothing opens after a cancel, and nothing is
                // remembered. The core kept a copy before this answer came;
                // it has no way to remove one entry's copy, so every copy
                // ends (core request: per-entry copy removal).
                opened.entry.close()
                if opened.faceIdUntilMs != nil {
                    endCopiesLeftByLateOpen()
                }
                return
            }
            noteOpened(entry.path)
            refreshCopies()
            let session = OpenSession(ref: entry, entry: opened.entry, view: opened.entry.view())
            sessions[session.id] = session
            pendingRoute = .entry(session.id)
            task.stage = .done
            keyTask = task
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: .seconds(1))
                guard let self, keyTask?.id == taskID else { return }
                keyTask = nil
            }
        case .failure(let error):
            handleKeyFailure(error, task: current, pin: pin)
        }
    }

    private func endCopiesLeftByLateOpen() {
        do {
            try copies.lockNow()
        } catch {
            copiesProblem = describe(error)
        }
        refreshCopies()
    }

    private func handleKeyFailure(_ error: Error, task: KeyTask?, pin: String) {
        let pawError = error as? PawError
        switch pawError {
        case .WrongPin(let triesLeft):
            heldPin = nil
            card?.pinTriesLeft = triesLeft
        case .PinBlocked:
            heldPin = nil
            card?.pinTriesLeft = 0
        default:
            break
        }
        guard var task else { return }
        switch pawError {
        case .WrongPin(let triesLeft):
            self.pin = ""
            task.stage = .wrongPin(triesLeft: Int(triesLeft))
        case .PinBlocked:
            self.pin = ""
            task.stage = .locked
        case .NotForThisKey:
            keyTask = nil
            path.append(.anotherKey(task.entry.path))
            return
        case .Decrypt where task.touchRequested:
            task.stage = .failed(
                problem: "Your key wasn't touched in time",
                nextStep: "Nothing was opened. Try again, and touch the gold part of your key while it blinks.")
        default:
            task.stage = .failed(problem: "\(task.entry.name) didn't open", nextStep: describe(error))
        }
        keyTask = task
    }

    /// Opens from the entry's Face ID copy, with no key sheet. Falls back
    /// to the key when the copy has ended.
    private func openWithFaceId(_ entry: EntryRef) {
        guard faceIdOpening == nil else { return }
        faceIdOpening = entry.path
        let store = store
        let copies = copies
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
    func lockNow() {
        let ended = liveCopies.count
        do {
            try copies.lockNow()
        } catch {
            copiesProblem = describe(error)
        }
        heldPin = nil
        lockGeneration += 1
        pinGeneration += 1
        keyTask = nil
        pendingRoute = nil
        path = []
        pathChanged()
        Clipboard.clearIfOurs()
        refreshCopies()
        toast = ended == 1 ? "Locked. 1 entry now needs your key." : "Locked. \(ended) entries now need your key."
    }

    func scenePhaseChanged(_ phase: ScenePhase) {
        switch phase {
        case .background:
            backgroundedAt = backgroundedAt ?? Date()
        case .active:
            if let away = backgroundedAt, Date().timeIntervalSince(away) > Self.pinForgetAfter, heldPin != nil {
                heldPin = nil
                pinGeneration += 1
                pinForgotten = true
            }
            backgroundedAt = nil
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

/// The lock and PIN generations a call started in.
private struct Generations: Sendable {
    let lock: Int
    let pin: Int
}

/// Hooks into the simulated hardware, for the debug controls.
struct SimulatorControls {
    let advanceClock: (UInt64) -> Void
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
    /// "key ID 1234 5678 9ABC DEF0", grouped in fours to read aloud.
    var spoken: String {
        func grouped(_ hex: String) -> String {
            stride(from: 0, to: hex.count, by: 4).map { offset -> String in
                let start = hex.index(hex.startIndex, offsetBy: offset)
                let end = hex.index(start, offsetBy: 4, limitedBy: hex.endIndex) ?? hex.endIndex
                return String(hex[start..<end])
            }.joined(separator: " ")
        }
        switch self {
        case .keyId(let id): return "key ID \(grouped(id))"
        case .fingerprint(let fingerprint): return "key \(grouped(fingerprint))"
        case .hidden: return "a hidden recipient"
        }
    }
}

/// Times as the screens say them: "6:40 PM".
func clockTime(ms: Int64) -> String {
    Date(timeIntervalSince1970: TimeInterval(ms) / 1000).formatted(date: .omitted, time: .shortened)
}
