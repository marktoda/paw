import CryptoKit
import Foundation
import Observation
import SwiftUI
import UIKit

/// Screens about sync on the navigation stack (`Route.sync`).
enum SyncRoute: Hashable {
    /// Waiting changes paw will not send as they are.
    case heldBack
    /// What the git remote holds that paw will not check out.
    case refusedRemote
    /// The git remote's history was replaced.
    case replaced
    /// "Review who can open this folder", from a held-back change or a
    /// refresh that changed a `.gpg-id`.
    case review(folder: String)
}

/// The sync sheet while it is up: the status page, or a key task's stages.
/// One sheet for both, so going from one to the other never stacks sheets.
struct SyncSheet: Identifiable {
    enum Mode: Equatable {
        case status
        case key(kind: SyncKind, taskID: UUID, stage: KeySheetStage)
    }

    let id = UUID()
    var mode: Mode
}

/// What the pill on Home says. Worked out from what the core reports:
/// the queue, the held-back and refused lists, and the last task's result.
enum SyncPillState: Equatable {
    case working(String)
    case upToDate(checked: Date)
    /// Never checked from this iPhone: the next check needs the key.
    case needsKey
    /// Never checked from this iPhone; the GitHub sign-in can check.
    case notChecked
    /// Changes wait here. The card under the pill counts them, so the
    /// pill says only when paw last checked.
    case notSent(checked: Date?)
    case offline(checked: Date?)
    case failed(String)
    /// Stopped until the person looks: replaced history, a refused remote
    /// or held-back changes.
    case paused(String)

    var status: SyncPill.Status {
        switch self {
        case .working: .syncing
        case .upToDate: .synced
        case .needsKey: .needsKey
        case .notChecked: .syncing
        case .notSent: .checked
        case .offline: .offline
        case .failed, .paused: .failed
        }
    }

    func text(now: Date) -> String {
        switch self {
        case .working(let text): text
        case .upToDate(let checked): "Up to date \u{00B7} checked \(syncAgo(checked, now: now))"
        case .needsKey: "Updates need your key"
        case .notChecked: "Not checked yet"
        case .notSent(let checked?): "Checked \(syncAgo(checked, now: now))"
        case .notSent(nil): "Not checked yet"
        case .offline(let checked?): "Offline \u{00B7} last checked \(syncAgo(checked, now: now))"
        case .offline(nil): "Offline"
        case .failed(let text), .paused(let text): text
        }
    }
}

/// Entry paths and a hash of each one's file, to say what a sync changed.
/// The core's `IntegrateReport` names only the entries both sides changed
/// (core request: added, changed and removed entries in the report).
struct StoreSnapshot: Sendable {
    let hashes: [String: Data]

    /// Reads every entry's file. Call off the main thread.
    static func take(store: PassStore, repoDir: String) -> StoreSnapshot {
        let root = URL(fileURLWithPath: repoDir, isDirectory: true)
        var hashes: [String: Data] = [:]
        var pending = [store.home()]
        while let listing = pending.popLast() {
            for entry in listing.entries {
                if let data = try? Data(contentsOf: root.appendingPathComponent(entry.path + ".gpg")) {
                    hashes[entry.path] = Data(SHA256.hash(data: data))
                }
            }
            for folder in listing.folders {
                if let inner = try? store.folder(path: folder.path) {
                    pending.append(inner)
                }
            }
        }
        return StoreSnapshot(hashes: hashes)
    }
}

/// What a sync brought in, for the dismissible card on Home.
struct SyncSummary: Identifiable, Equatable {
    enum Origin: Equatable {
        case sync
        /// After taking the git remote's replaced history.
        case replaced
    }

    let id = UUID()
    let origin: Origin
    var added: [String] = []
    var changed: [String] = []
    var removed: [String] = []
    var keptBoth: [KeptBoth] = []
    var keptLocal: [String] = []
    var droppedDeletes: [String] = []
    /// Folders whose `.gpg-id` changed ("" for the top of the store).
    var recipientsChanged: [String] = []
    /// Entries whose Face ID copy ended because their text changed.
    var endedCopies: [String] = []

    init(origin: Origin, report: IntegrateReport, before: StoreSnapshot?, after: StoreSnapshot?) {
        self.origin = origin
        keptBoth = report.keptBoth
        keptLocal = report.keptLocal
        droppedDeletes = report.droppedDeletes
        recipientsChanged = report.recipientsChanged.map { change in
            let folder = change.path.split(separator: "/").dropLast().joined(separator: "/")
            return folder
        }
        guard let before, let after else { return }
        // Entries the report already explains are said there instead.
        var explained = Set(keptLocal + droppedDeletes)
        for pair in keptBoth {
            explained.insert(pair.entry)
            explained.insert(pair.copy)
        }
        let sorted = { (paths: [String]) in
            paths.filter { !explained.contains($0) }.sorted { $0.localizedStandardCompare($1) == .orderedAscending }
        }
        added = sorted(after.hashes.keys.filter { before.hashes[$0] == nil })
        removed = sorted(before.hashes.keys.filter { after.hashes[$0] == nil })
        changed = sorted(after.hashes.compactMap { path, hash in
            guard let old = before.hashes[path], old != hash else { return nil }
            return path
        })
    }

    var entryCount: Int {
        added.count + changed.count + removed.count
    }

    /// Something in it asks for a decision: two versions of one password,
    /// a delete that didn't happen, a changed list of keys.
    var needsALook: Bool {
        !(keptBoth.isEmpty && keptLocal.isEmpty && droppedDeletes.isEmpty && recipientsChanged.isEmpty)
    }

    var isEmpty: Bool {
        entryCount == 0 && keptBoth.isEmpty && keptLocal.isEmpty && droppedDeletes.isEmpty
            && recipientsChanged.isEmpty
    }

    var title: String {
        let count = entryCount
        if count > 0 {
            if origin == .replaced {
                return count == 1 ? "1 password updated from the new store" : "\(count) passwords updated from the new store"
            }
            return count == 1 ? "1 password changed on your other devices" : "\(count) passwords changed on your other devices"
        }
        if let pair = keptBoth.first {
            return "Two versions of \(EntryRef(path: pair.entry).name) were kept"
        }
        if !keptLocal.isEmpty || !droppedDeletes.isEmpty {
            return "Both sides changed the same password"
        }
        if let folder = recipientsChanged.first {
            return "Who can open \(folder.isEmpty ? "your store" : folder) changed"
        }
        return "Your store was updated"
    }
}

/// Wraps a value that crosses from the sync queue to the main actor once.
private struct SyncHandoff<Value>: @unchecked Sendable {
    let value: Value
}

/// Hands a core task's stage changes to the main queue, in order.
private final class SyncTaskListener: KeyTaskListener, @unchecked Sendable {
    private let changed: @Sendable () -> Void

    init(changed: @escaping @Sendable () -> Void) {
        self.changed = changed
    }

    func stateChanged(state: KeyTaskState) {
        DispatchQueue.main.async(execute: changed)
    }
}

/// Sending and checking for changes: the pill, the sync sheet and the
/// screens after a sync. The core does the sync, in its own thread, on one
/// card-signed login; this keeps what the screens show, and never starts
/// the network or the card without a tap. The PIN goes from the sheet's
/// field straight to the core task.
@MainActor @Observable
final class SyncModel {
    let sync: StoreSync
    let repoDir: String
    let address: String

    var sheet: SyncSheet?
    /// What is typed in the sheet's PIN field: handed to the core and
    /// cleared at once, and whenever the sheet moves on.
    var pinEntry = ""
    private(set) var running: SyncKind?
    private(set) var progress: TaskProgress?
    private(set) var lastChecked: Date?
    private(set) var failure: SyncFailure?
    private(set) var replaced: Replaced?
    private(set) var refused: [RefusedPath] = []
    private(set) var heldBack: [HeldBack] = []
    /// Held-back changes "Re-encrypt" can do now: their folder's new list
    /// is accepted and paw has every key on it.
    private(set) var reencryptReady = 0
    /// Why held-back changes can't be re-encrypted, by entry: what the core
    /// sees now, and what the last re-encrypt found when it tried.
    private(set) var reencryptLeft: [String: LeftReason] = [:]
    var summary: SyncSummary?
    /// Taking a replaced history is under way, or why it failed.
    private(set) var accepting = false
    var acceptProblem: String?
    /// The simulated host's tip and logins, and what its debug controls
    /// last did, for UI tests. Empty on a device.
    private(set) var simSummary = ""
    private(set) var simStatus = ""
    /// The GitHub sign-in, which checks for changes without the key when
    /// it is for this store's repository.
    @ObservationIgnored var gitHub: GitHubAccess?
    /// Said in the key sheet when a check fell back to the key.
    private(set) var keyPathNote: String?
    /// The last background check (`BackgroundRefresh`), for the status page.
    private(set) var backgroundRun: BackgroundRun?

    @ObservationIgnored private weak var app: AppModel?
    @ObservationIgnored private var tasks: [UUID: KeyTask] = [:]
    @ObservationIgnored private var before: StoreSnapshot?
    /// The key task has had its touch: "Connecting" is over.
    @ObservationIgnored private var signedIn = false
    /// Where to go once the sheet has gone.
    @ObservationIgnored private var pendingRoute: Route?
    @ObservationIgnored private var pendingSettings = false
    @ObservationIgnored private var backgroundWatch: NSObjectProtocol?
    /// What the last re-encrypt left, and why (only what it found trying).
    @ObservationIgnored private var lastLeft: [LeftHeldBack] = []
    /// Re-encrypt tasks not yet ended, however the sheet let them go.
    @ObservationIgnored private var reencryptTasks: Set<UUID> = []
    private let workQueue = DispatchQueue(label: "paw-sync", qos: .userInitiated)

    init(sync: StoreSync, repoDir: String) {
        self.sync = sync
        self.repoDir = repoDir
        address = sync.address()
        let seen = UserDefaults.standard.double(forKey: Self.checkedKey(repoDir))
        lastChecked = seen > 0 ? Date(timeIntervalSince1970: seen) : nil
        #if targetEnvironment(simulator)
        SimRemoteHandle.shared.storeReady(repoDir: repoDir)
        refreshSimSummary()
        #endif
        recomputeLocal()
        backgroundRun = BackgroundRefresh.lastRun()
        // A background run that ends while paw is on screen (or coming
        // back to it); one that ends in the background waits for the next
        // `cameForward`.
        backgroundWatch = NotificationCenter.default.addObserver(
            forName: BackgroundRefresh.finished, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, let app = self.app, UIApplication.shared.applicationState != .background else { return }
                self.cameForward(app: app)
            }
        }
    }

    /// Where the time of the last check is kept; the background refresh
    /// writes it too.
    nonisolated static func checkedKey(_ repoDir: String) -> String {
        "pawSyncLastChecked:\(repoDir)"
    }

    var host: String {
        SetupModel.host(of: address)
    }

    // MARK: What the pill says

    func pillState(unpushed: Int) -> SyncPillState {
        if let running {
            return .working(workingText(running, unpushed: unpushed))
        }
        if replaced != nil || !refused.isEmpty {
            return .paused("Sync paused \u{00B7} Review")
        }
        if !heldBack.isEmpty {
            return .paused(heldBack.count == 1 ? "1 change held back \u{00B7} Review" : "\(heldBack.count) changes held back \u{00B7} Review")
        }
        if let failure, failure.kind != .cancelled {
            return failure.kind == .offline ? .offline(checked: lastChecked) : .failed(failure.pill)
        }
        if unpushed > 0 {
            return .notSent(checked: lastChecked)
        }
        if let lastChecked {
            return .upToDate(checked: lastChecked)
        }
        return checksWithSignIn ? .notChecked : .needsKey
    }

    private func workingText(_ kind: SyncKind, unpushed: Int) -> String {
        let base: String
        switch kind {
        case .send: base = unpushed == 1 ? "Sending 1 change\u{2026}" : "Sending \(unpushed) changes\u{2026}"
        case .refresh: base = "Checking for changes\u{2026}"
        case .reencrypt: base = "Re-encrypting\u{2026}"
        }
        guard let progress, progress.total > 0 else { return base }
        return "\(base) \(progress.done) of \(progress.total)"
    }

    /// A tap on the pill: the status sheet, or with the disc, straight to
    /// the key sheet that updates the store (design.md 2.3).
    func pillTapped(app: AppModel) {
        let unpushed = app.edits?.unpushed.count ?? 0
        let state = pillState(unpushed: unpushed)
        if state == .needsKey || state == .notChecked {
            check(app: app)
        } else {
            showStatus()
        }
    }

    func showStatus() {
        guard sheet == nil else { return }
        recomputeLocal()
        sheet = SyncSheet(mode: .status)
    }

    /// Whether "Send" is offered: never for a change that is held back, and
    /// not while the remote needs looking at first.
    func canSend(unpushed: Int) -> Bool {
        unpushed > 0 && heldBack.isEmpty && replaced == nil && refused.isEmpty
    }

    /// The local state again, without the network or the key: on coming to
    /// the foreground and after each change to the queue.
    func recomputeLocal() {
        guard running == nil, !accepting else { return }
        heldBack = (try? sync.heldBack()) ?? []
        refused = (try? sync.refusedRemote()) ?? []
        recomputeReencrypt()
    }

    /// `reencryptReady` and `reencryptLeft` from the core, without the key.
    private func recomputeReencrypt() {
        let check: ReencryptCheck? = heldBack.isEmpty ? nil : (try? sync.reencryptCheck())
        reencryptReady = Int(check?.ready ?? 0)
        let held = Set(heldBack.map(\.entry))
        var left: [String: LeftReason] = [:]
        for item in lastLeft + (check?.left ?? []) where held.contains(item.entry) {
            left[item.entry] = item.reason
        }
        reencryptLeft = left
    }

    /// "Re-encrypt 3 changes": the key sheet, then the core's task that
    /// opens each held-back change with the key (a touch each) and encrypts
    /// it for its folder's new list. Only ever started by a tap.
    func startReencrypt(app: AppModel) {
        guard reencryptReady > 0 else { return }
        start(.reencrypt, app: app)
    }

    // MARK: Running a send or a check

    /// "Send N changes" or "Check for changes": the key sheet, then the
    /// core's sign-in task. What the store holds is read first, off the main
    /// thread, to say afterwards what changed.
    func start(_ kind: SyncKind, app: AppModel) {
        guard running == nil, !accepting else { return }
        self.app = app
        failure = nil
        running = kind
        progress = nil
        signedIn = false
        pinEntry = ""
        if kind == .reencrypt {
            lastLeft = []
        }
        let taskID = UUID()
        let mode = SyncSheet.Mode.key(kind: kind, taskID: taskID, stage: .busy)
        if sheet == nil {
            sheet = SyncSheet(mode: mode)
        } else {
            sheet?.mode = mode
        }
        let store = app.store
        let repoDir = repoDir
        workQueue.async { [weak self] in
            let snapshot = SyncHandoff(value: StoreSnapshot.take(store: store, repoDir: repoDir))
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.launch(kind, taskID: taskID, before: snapshot.value)
                }
            }
        }
    }

    private func launch(_ kind: SyncKind, taskID: UUID, before: StoreSnapshot) {
        // Cancelled while the store was being read.
        guard case .key(_, let current, _)? = sheet?.mode, current == taskID, let app else { return }
        self.before = before
        let listener = SyncTaskListener { [weak self] in
            MainActor.assumeIsolated {
                self?.taskChanged(taskID)
            }
        }
        switch kind {
        case .send: tasks[taskID] = sync.sendWithKey(port: app.keyPort, listener: listener)
        case .refresh: tasks[taskID] = sync.refreshWithKey(port: app.keyPort, listener: listener)
        case .reencrypt:
            reencryptTasks.insert(taskID)
            tasks[taskID] = sync.reencryptWithKey(port: app.keyPort, listener: listener)
        }
        BackgroundTime.hold("Sync", while: tasks[taskID])
        taskChanged(taskID)
    }

    func continueWithPin() {
        guard case .key(_, let id, let stage)? = sheet?.mode, let task = tasks[id], !pinEntry.isEmpty,
              Self.asksForPin(stage) else { return }
        task.supplyPin(pin: Data(pinEntry.utf8))
        pinEntry = ""
    }

    /// "Try again" in the key sheet: a new task, which opens at the first
    /// stage still needed (Touch if the PIN is still held).
    func retry() {
        guard case .key(let kind, let id, _)? = sheet?.mode, let app else { return }
        tasks[id]?.cancel()
        running = nil
        start(kind, app: app)
    }

    /// "Try again" on the status page, after a failure.
    func retryFailed(app: AppModel) {
        guard let failure else { return }
        if failure.during == .refresh {
            check(app: app)
        } else if failure.during == .reencrypt {
            startReencrypt(app: app)
        } else {
            start(failure.during, app: app)
        }
    }

    /// Cancel, or the sheet swiped away. A command waiting for a touch
    /// cannot be stopped; the core throws its answer away. The queue is
    /// left as it was.
    func cancelSheet() {
        if case .key(let kind, let id, _)? = sheet?.mode {
            tasks[id]?.cancel()
            if running != nil {
                failure = SyncFailure.cancelled(during: kind, unpushed: app?.edits?.unpushed.count ?? 0)
            }
            running = nil
        }
        sheet = nil
        pinEntry = ""
        keyPathNote = nil
    }

    /// The queue after a re-encrypt ended, however it ended: it may have
    /// rewritten some, and after a cancel the core may still be writing the
    /// last one.
    private func reloadAfterReencrypt() {
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(300))
            guard let self, self.running == nil else { return }
            self.app?.reloadAfterWrite()
            self.recomputeLocal()
        }
    }

    /// The sheet went away: go where it said to.
    func sheetDismissed(app: AppModel) {
        if let route = pendingRoute {
            pendingRoute = nil
            app.path.append(route)
        }
        if pendingSettings {
            pendingSettings = false
            app.showsSettings = true
        }
    }

    /// Closes the sheet, then shows `route`.
    func go(to route: Route) {
        pendingRoute = route
        cancelSheet()
    }

    /// Closes the sheet, then opens Settings.
    func openSettings() {
        pendingSettings = true
        cancelSheet()
    }

    private func taskChanged(_ id: UUID) {
        guard let task = tasks[id] else { return }
        let state = task.state()
        if state.isFinal, reencryptTasks.remove(id) != nil {
            reloadAfterReencrypt()
        }
        guard var sheet, case .key(let kind, let current, _) = sheet.mode, current == id else {
            // A task the sheet let go of: forget it once it ends.
            if state.isFinal { tasks[id] = nil }
            return
        }
        if state.isFinal {
            tasks[id] = nil
        }
        let unpushed = app?.edits?.unpushed.count ?? 0
        let stage: KeySheetStage
        switch state {
        case .plugIn:
            stage = .plugIn
        case .pin(let triesLeft, let forgotten, let lastError):
            let tries = Int(triesLeft ?? 3)
            switch lastError {
            case nil:
                stage = .pin(triesLeft: tries, forgotten: forgotten)
            case .WrongPin(let left)?:
                stage = .wrongPin(triesLeft: Int(left))
            case let error?:
                let words = syncKeyWords(error, during: kind, unpushed: unpushed)
                stage = .pinAgain(triesLeft: tries, problem: words.problem, nextStep: words.nextStep)
            }
        case .busy:
            stage = .busy
        case .touch:
            signedIn = true
            stage = .touch
        case .working(let progress):
            if kind == .reencrypt {
                // The key's answer clears it; the count goes on.
                if let progress { self.progress = progress }
                stage = .working(message: workingMessage(kind, unpushed: unpushed), progress: nil)
            } else {
                self.progress = progress
                stage = .working(message: workingMessage(kind, unpushed: unpushed), progress: progress.map(Self.progressLine))
            }
        case .done:
            stage = .done
            finished(kind, sheetID: sheet.id)
        case .failed(.Reader(let detail)):
            if failedElsewhere(kind, unpushed: unpushed) {
                return
            }
            let words = syncKeyWords(.Reader(detail: detail), during: kind, unpushed: unpushed)
            stage = .failed(problem: words.problem, nextStep: words.nextStep)
            running = nil
        case .failed(.Cancelled), .cancelled:
            cancelSheet()
            return
        case .failed(.PinBlocked):
            stage = .locked
            running = nil
        case .failed(let error):
            let words = syncKeyWords(error, during: kind, unpushed: unpushed)
            stage = .failed(problem: words.problem, nextStep: words.nextStep)
            running = nil
            #if targetEnvironment(simulator)
            SimRemoteHandle.shared.syncFailed()
            #endif
        }
        sheet.mode = .key(kind: kind, taskID: id, stage: stage)
        if !Self.asksForPin(stage) {
            pinEntry = ""
        }
        self.sheet = sheet
    }

    private func workingMessage(_ kind: SyncKind, unpushed: Int) -> String {
        if kind == .reencrypt {
            return "Re-encrypting\u{2026}"
        }
        if !signedIn, progress == nil {
            return "Connecting to \(host)\u{2026}"
        }
        switch kind {
        case .send:
            return unpushed == 1 ? "Sending 1 change\u{2026}" : "Sending \(unpushed) changes\u{2026}"
        case .refresh, .reencrypt:
            return "Checking for changes\u{2026}"
        }
    }

    private static func progressLine(_ progress: TaskProgress) -> String {
        "\(Int(progress.done).formatted()) of \(Int(progress.total).formatted())"
    }

    private static func asksForPin(_ stage: KeySheetStage) -> Bool {
        switch stage {
        case .pin, .wrongPin, .pinAgain: true
        default: false
        }
    }

    /// The task failed for a reason that is not the key's: the sheet turns
    /// into the status page, with the failure said inline. `false` if it
    /// was the key's after all (the reader itself failed).
    private func failedElsewhere(_ kind: SyncKind, unpushed: Int) -> Bool {
        let error: Error
        do {
            if kind == .reencrypt {
                _ = try sync.lastReencrypt()
            } else {
                _ = try sync.lastTaskResult()
            }
            return false
        } catch let found {
            error = found
        }
        running = nil
        #if targetEnvironment(simulator)
        SimRemoteHandle.shared.syncFailed()
        refreshSimSummary()
        #endif
        if let error = error as? SyncError {
            failure = SyncFailure(error, during: kind, unpushed: unpushed, host: host)
        } else {
            failure = SyncFailure(
                kind: .other, during: kind, problem: "Couldn't sync", nextStep: SyncFailure.waiting(unpushed),
                details: [describe(error)], pill: "Sync failed")
        }
        recomputeLocal()
        pinEntry = ""
        sheet?.mode = .status
        return true
    }

    /// The core's sync went through. The sheet shows Done for a second and
    /// closes, unless what came back needs looking at (replaced history,
    /// held-back changes): then it turns into the status page that says so.
    private func finished(_ kind: SyncKind, sheetID: UUID) {
        running = nil
        let needsLook = kind == .reencrypt
            ? reencrypted((try? sync.lastReencrypt()) ?? nil)
            : absorb((try? sync.lastTaskResult()) ?? nil, kind: kind)
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard let self, self.sheet?.id == sheetID, case .key(_, _, .done)? = self.sheet?.mode else { return }
            if needsLook {
                self.sheet?.mode = .status
            } else {
                self.sheet = nil
            }
        }
    }

    /// What a sync that went through brought back, said on the pill, the
    /// status page and the card on Home. `true` if it needs looking at.
    private func absorb(_ report: SyncReport?, kind: SyncKind) -> Bool {
        lastLeft = []
        lastChecked = Date()
        UserDefaults.standard.set(Date().timeIntervalSince1970, forKey: Self.checkedKey(repoDir))
        var needsLook = false
        if let report {
            switch report.state {
            case .remoteReplaced(let found):
                replaced = found
                needsLook = true
            case .waiting:
                heldBack = (try? sync.heldBack()) ?? []
                needsLook = kind == .send && !heldBack.isEmpty
            case .upToDate:
                replaced = nil
                if kind == .send, report.pushed > 0 {
                    app?.toast = "Sent"
                }
            }
            integrated(report.integration, origin: .sync)
        } else {
            app?.reloadAfterWrite()
        }
        refused = (try? sync.refusedRemote()) ?? []
        #if targetEnvironment(simulator)
        refreshSimSummary()
        #endif
        return needsLook
    }

    /// A re-encrypt that ran to the end (`taskChanged` reads the queue
    /// again). Always back to the status page after: what is left held back
    /// is said there, and Send is there once none is.
    private func reencrypted(_ report: ReencryptReport?) -> Bool {
        lastLeft = report?.left ?? []
        if let report, report.reencrypted > 0 {
            app?.toast = "Re-encrypted. Not sent yet."
        }
        return true
    }

    // MARK: After a background check

    /// paw is on screen again: what background checks brought in shows in
    /// the list, and what they left for the person comes up through a
    /// check with the sign-in (no key), as a check from the pill would.
    func cameForward(app: AppModel) {
        self.app = app
        backgroundRun = BackgroundRefresh.lastRun()
        let seen = UserDefaults.standard.double(forKey: Self.checkedKey(repoDir))
        if seen > 0 {
            lastChecked = Date(timeIntervalSince1970: seen)
        }
        guard running == nil, !accepting else { return }
        let pending = BackgroundRefresh.takePending()
        if pending.changed {
            app.reloadAfterWrite()
            recomputeLocal()
        }
        if pending.needsLook, checksWithSignIn {
            check(app: app)
        }
    }

    // MARK: Checking with the GitHub sign-in

    /// The GitHub sign-in is for this store's repository: checking needs
    /// no key.
    var checksWithSignIn: Bool {
        gitHub?.covers(address: address) ?? false
    }

    /// "Check for changes": with the GitHub sign-in when it covers this
    /// store, else with the key.
    func check(app: AppModel) {
        if checksWithSignIn {
            checkWithSignIn(app: app)
        } else {
            start(.refresh, app: app)
        }
    }

    /// Fetches over HTTPS with the sign-in and integrates as the key's
    /// check does, with no key sheet. Sends nothing. If the sign-in has
    /// ended, the check goes on with the key, and the key sheet says why.
    private func checkWithSignIn(app: AppModel) {
        guard running == nil, !accepting, let gitHub else { return }
        self.app = app
        failure = nil
        running = .refresh
        progress = nil
        keyPathNote = nil
        let store = app.store
        let repoDir = repoDir
        let core = gitHub.core
        let time = BackgroundTime.begin("Sync")
        workQueue.async { [weak self] in
            let snapshot = StoreSnapshot.take(store: store, repoDir: repoDir)
            let result = Result { try core.refreshStore() }
            let stillSignedIn = (try? core.isSignedIn()) ?? false
            let handoff = SyncHandoff(value: (snapshot, result, stillSignedIn))
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    time.end()
                    self?.checkedWithSignIn(before: handoff.value.0, handoff.value.1, stillSignedIn: handoff.value.2)
                }
            }
        }
    }

    private func checkedWithSignIn(before: StoreSnapshot, _ result: Result<SyncReport, Error>, stillSignedIn: Bool) {
        running = nil
        gitHub?.reload()
        switch result {
        case .success(let report):
            self.before = before
            if absorb(report, kind: .refresh) {
                recomputeLocal()
                if sheet == nil {
                    sheet = SyncSheet(mode: .status)
                } else {
                    sheet?.mode = .status
                }
            }
        case .failure(let error):
            if !stillSignedIn, let app {
                start(.refresh, app: app)
                keyPathNote = "Your GitHub sign-in ended, so this check uses your key."
                return
            }
            let unpushed = app?.edits?.unpushed.count ?? 0
            if let error = error as? SyncError {
                failure = SyncFailure(error, during: .refresh, unpushed: unpushed, host: "github.com")
            } else {
                failure = SyncFailure(
                    kind: .other, during: .refresh, problem: "paw couldn't check for changes.",
                    nextStep: SyncFailure.waiting(unpushed), details: [describe(error)], pill: "Check failed")
            }
            recomputeLocal()
        }
    }

    /// After an integrate: the lists read the store again, Face ID copies
    /// of entries whose text changed end (the core finds them), and the
    /// card on Home says what changed.
    private func integrated(_ report: IntegrateReport, origin: SyncSummary.Origin) {
        let before = before
        self.before = nil
        let repoDir = repoDir
        let copies = app?.copies
        workQueue.async { [weak self] in
            let store = try? PassStore(dir: repoDir)
            let after = store.map { StoreSnapshot.take(store: $0, repoDir: repoDir) }
            var summary = SyncSummary(origin: origin, report: report, before: before, after: after)
            if let copies, let store {
                for copy in (try? copies.list()) ?? [] {
                    if case .stale? = try? store.faceIdStatus(copies: copies, entry: copy.entry) {
                        summary.endedCopies.append(copy.entry)
                    }
                }
            }
            let handoff = SyncHandoff(value: summary)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self else { return }
                    self.app?.reloadAfterWrite()
                    if !handoff.value.isEmpty || !handoff.value.endedCopies.isEmpty {
                        self.summary = handoff.value
                    }
                    self.recomputeLocal()
                }
            }
        }
    }

    // MARK: Replaced history

    /// Takes the git remote's new history, keeping this iPhone's waiting
    /// changes (replayed onto it) or discarding them. Local work only: no
    /// key, no network.
    func acceptReplaced(keepChanges: Bool, app: AppModel) {
        guard replaced != nil, running == nil, !accepting else { return }
        self.app = app
        accepting = true
        acceptProblem = nil
        let sync = sync
        let store = app.store
        let repoDir = repoDir
        let time = BackgroundTime.begin("Sync")
        workQueue.async { [weak self] in
            let snapshot = StoreSnapshot.take(store: store, repoDir: repoDir)
            let result = Result { try sync.acceptReplacedRemote(keepChanges: keepChanges) }
            let handoff = SyncHandoff(value: (snapshot, result))
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    time.end()
                    self?.accepted(handoff.value.0, handoff.value.1, keepChanges: keepChanges)
                }
            }
        }
    }

    private func accepted(_ snapshot: StoreSnapshot, _ result: Result<IntegrateReport, Error>, keepChanges: Bool) {
        accepting = false
        switch result {
        case .success(let report):
            replaced = nil
            failure = nil
            before = snapshot
            integrated(report, origin: .replaced)
            if let app {
                app.path.removeAll { $0 == .sync(.replaced) }
                app.toast = keepChanges ? "Using the new store. Your changes are kept." : "Using the new store."
            }
        case .failure(let error):
            acceptProblem = (error as? SyncError).map { describeEditError($0) } ?? describe(error)
        }
    }

    /// "Not now" on the replaced screen: sync stays paused.
    func notNow(app: AppModel) {
        app.path.removeAll { $0 == .sync(.replaced) }
    }

    // MARK: The simulated host

    #if targetEnvironment(simulator)
    func refreshSimSummary() {
        simSummary = SimRemoteHandle.shared.summary
    }

    /// Runs off the main thread: a rewrite starts a new host.
    func applySimChange(_ change: SimRemoteChange) {
        simStatus = "Working\u{2026}"
        workQueue.async { [weak self] in
            let said: String
            do {
                said = try SimRemoteHandle.shared.apply(change)
            } catch {
                said = "Failed: \(describe(error))"
            }
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.simStatus = said
                    self?.refreshSimSummary()
                }
            }
        }
    }

    func setSimRefusesPushes(_ refuse: Bool) {
        SimRemoteHandle.shared.setRefusesPushes(refuse)
        simStatus = refuse ? "Refusing pushes." : "Accepting pushes."
    }

    func simDropNextConnection() {
        SimRemoteHandle.shared.dropNextConnection()
        simStatus = "The next connection drops half way."
    }

    func setSimChangedHostKey(_ changed: Bool) {
        SimRemoteHandle.shared.setChangedHostKey(changed)
        simStatus = changed ? "Presenting another host key." : "Presenting the usual host key."
    }
    #endif
}
