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
        /// The key sheet; its stage is `SyncModel.keyTask`'s.
        case key(kind: SyncKind)
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

    init(origin: Origin, report: IntegrateReport) {
        self.origin = origin
        keptBoth = report.keptBoth
        keptLocal = report.keptLocal
        droppedDeletes = report.droppedDeletes
        recipientsChanged = report.recipientsChanged.map { change in
            let folder = change.path.split(separator: "/").dropLast().joined(separator: "/")
            return folder
        }
        // Entries the report already explains are said there instead.
        var explained = Set(keptLocal + droppedDeletes)
        for pair in keptBoth {
            explained.insert(pair.entry)
            explained.insert(pair.copy)
        }
        let sorted = { (paths: [String]) in
            paths.filter { !explained.contains($0) }.sorted { $0.localizedStandardCompare($1) == .orderedAscending }
        }
        added = sorted(report.added)
        changed = sorted(report.changed)
        removed = sorted(report.removed)
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
    /// The key sheet's task: a send, a check or a re-encrypt.
    let keyTask: KeyTaskDriver
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
    /// The key task has had its touch: "Connecting" is over.
    @ObservationIgnored private var signedIn = false
    /// Where to go once the sheet has gone.
    @ObservationIgnored private var pendingRoute: Route?
    @ObservationIgnored private var pendingSettings = false
    @ObservationIgnored private var backgroundWatch: NSObjectProtocol?
    /// What the last re-encrypt left, and why (only what it found trying).
    @ObservationIgnored private var lastLeft: [LeftHeldBack] = []
    /// What came back needs looking at: after Done, the sheet turns into
    /// the status page instead of closing.
    @ObservationIgnored private var doneNeedsLook = false
    private let workQueue = DispatchQueue(label: "paw-sync", qos: .userInitiated)

    init(sync: StoreSync, repoDir: String) {
        self.sync = sync
        self.repoDir = repoDir
        keyTask = KeyTaskDriver()
        address = sync.address()
        let seen = UserDefaults.standard.double(forKey: Self.checkedKey(repoDir))
        lastChecked = seen > 0 ? Date(timeIntervalSince1970: seen) : nil
        #if targetEnvironment(simulator)
        SimRemoteHandle.shared.storeReady(repoDir: repoDir)
        refreshSimSummary()
        #endif
        recomputeLocal()
        backgroundRun = BackgroundRefresh.lastRun()
        keyTask.owner = self
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
        storeAddressHost(address: address)
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
    /// core's sign-in task.
    func start(_ kind: SyncKind, app: AppModel) {
        guard running == nil, !accepting else { return }
        self.app = app
        let mode = SyncSheet.Mode.key(kind: kind)
        if sheet == nil {
            sheet = SyncSheet(mode: mode)
        } else {
            sheet?.mode = mode
        }
        keyTask.reader = app.reader
        let sync = sync
        let port = app.keyPort
        keyTask.start { (listener: KeyTaskListener) -> KeyTask in
            let task: KeyTask = switch kind {
            case .send: sync.sendWithKey(port: port, listener: listener)
            case .refresh: sync.refreshWithKey(port: port, listener: listener)
            case .reencrypt: sync.reencryptWithKey(port: port, listener: listener)
            }
            BackgroundTime.hold("Sync", while: task)
            return task
        }
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
        if case .key(let kind)? = sheet?.mode {
            keyTask.cancel()
            if running != nil {
                failure = SyncFailure.cancelled(during: kind, unpushed: app?.edits?.unpushed.count ?? 0)
            }
            running = nil
        }
        sheet = nil
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

    /// The task failed for a reason that is not the key's: the sheet turns
    /// into the status page, with the failure said inline.
    private func syncFailed(_ error: SyncError, kind: SyncKind, unpushed: Int) {
        running = nil
        #if targetEnvironment(simulator)
        SimRemoteHandle.shared.syncFailed()
        refreshSimSummary()
        #endif
        failure = SyncFailure(error, during: kind, unpushed: unpushed, host: host)
        recomputeLocal()
        sheet?.mode = .status
    }

    /// The core's sync went through. The sheet shows Done for a second and
    /// closes, unless what came back needs looking at (replaced history,
    /// held-back changes): then it turns into the status page that says so.
    private func finished(_ kind: SyncKind, outcome: KeyOutcome) {
        running = nil
        switch outcome {
        case .reencrypted(let report):
            doneNeedsLook = reencrypted(report)
        case .synced(let report):
            doneNeedsLook = absorb(report, kind: kind)
        default:
            doneNeedsLook = kind == .reencrypt ? reencrypted(nil) : absorb(nil, kind: kind)
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

    /// A re-encrypt that ran to the end (`reloadAfterReencrypt` reads the
    /// queue again). Always back to the status page after: what is left held back
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
        let core = gitHub.core
        let time = BackgroundTime.begin("Sync")
        workQueue.async { [weak self] in
            let result = Result { try core.refreshStore() }
            let stillSignedIn = (try? core.isSignedIn()) ?? false
            let handoff = Handoff(value: (result, stillSignedIn))
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    time.end()
                    self?.checkedWithSignIn(handoff.value.0, stillSignedIn: handoff.value.1)
                }
            }
        }
    }

    private func checkedWithSignIn(_ result: Result<SyncReport, Error>, stillSignedIn: Bool) {
        running = nil
        gitHub?.reload()
        switch result {
        case .success(let report):
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
        let repoDir = repoDir
        let copies = app?.copies
        workQueue.async { [weak self] in
            let store = try? PassStore(dir: repoDir)
            var summary = SyncSummary(origin: origin, report: report)
            if let copies, let store {
                for copy in (try? copies.list()) ?? [] {
                    if case .stale? = try? store.faceIdStatus(copies: copies, entry: copy.entry) {
                        summary.endedCopies.append(copy.entry)
                    }
                }
            }
            let handoff = Handoff(value: summary)
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
        let time = BackgroundTime.begin("Sync")
        workQueue.async { [weak self] in
            let result = Handoff(value: Result { try sync.acceptReplacedRemote(keepChanges: keepChanges) })
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    time.end()
                    self?.accepted(result.value, keepChanges: keepChanges)
                }
            }
        }
    }

    private func accepted(_ result: Result<IntegrateReport, Error>, keepChanges: Bool) {
        accepting = false
        switch result {
        case .success(let report):
            replaced = nil
            failure = nil
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

extension SyncModel: KeyTaskOwner {
    private var keyKind: SyncKind {
        if case .key(let kind)? = sheet?.mode { return kind }
        return .refresh
    }

    private var unpushed: Int {
        app?.edits?.unpushed.count ?? 0
    }

    func keyWords(_ error: KeyError, pinAskedAgain: Bool) -> KeyErrorWords {
        syncKeyWords(error, during: keyKind, unpushed: unpushed)
    }

    func workingStage(_ progress: TaskProgress?) -> KeySheetStage {
        let kind = keyKind
        if kind == .reencrypt {
            // The key's answer clears it; the count goes on.
            if let progress { self.progress = progress }
            return .working(message: workingMessage(kind, unpushed: unpushed), progress: nil)
        }
        self.progress = progress
        return .working(message: workingMessage(kind, unpushed: unpushed), progress: progress.map(Self.progressLine))
    }

    func attachWords(_ problem: KeyReadProblem) -> KeyErrorWords {
        AppModel.readProblemWords(problem)
    }

    /// A send, check or re-encrypt starts, or starts again on Try again.
    func keyTaskStarting() {
        let kind = keyKind
        failure = nil
        running = kind
        progress = nil
        signedIn = false
        if kind == .reencrypt {
            lastLeft = []
        }
    }

    func keyStageShown(_ stage: KeySheetStage) {
        if stage == .touch {
            signedIn = true
        }
    }

    func keyTaskEnded(_ end: KeyTaskEnd) -> KeySheetStage? {
        if case .reencrypt = end.purpose {
            reloadAfterReencrypt()
        }
        guard case .key(let kind)? = sheet?.mode else { return nil }
        switch end.result {
        case .done(let outcome):
            finished(kind, outcome: outcome)
            return .done
        case .syncFailed(let error):
            syncFailed(error, kind: kind, unpushed: unpushed)
            return nil
        case .setupFailed, .cancelled:
            cancelSheet()
            return nil
        case .failed(let error):
            running = nil
            #if targetEnvironment(simulator)
            if error != .PinBlocked {
                SimRemoteHandle.shared.syncFailed()
            }
            #endif
            return standardStage(end.result)
        }
    }

    /// A task the sheet let go of has ended: a re-encrypt may have
    /// rewritten some changes.
    func letGoTaskEnded(_ end: KeyTaskEnd) {
        if case .reencrypt = end.purpose {
            reloadAfterReencrypt()
        }
    }

    /// Done has shown for a second: close, unless what came back needs
    /// looking at (replaced history, held-back changes); then the sheet
    /// turns into the status page that says so.
    func doneShown() {
        guard case .key? = sheet?.mode else { return }
        if doneNeedsLook {
            sheet?.mode = .status
        } else {
            sheet = nil
        }
    }
}
