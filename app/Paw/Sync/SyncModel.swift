import Foundation
import Observation
import SwiftUI

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

    let id: UUID
    let mode: Mode
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
        recipientsChanged = report.recipientsChanged.map { gpgIdFolder(path: $0.path) }
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
            return whoCanOpenChanged(folder)
        }
        return "Your store was updated"
    }
}

/// When paw last found the store as the server has it, by a check with
/// the key or the sign-in, in the foreground or the background. Kept per
/// store in the app's own defaults, which can be read while the iPhone is
/// locked.
enum LastChecked {
    static func read(repoDir: String) -> Date? {
        let seen = UserDefaults.standard.double(forKey: key(repoDir))
        return seen > 0 ? Date(timeIntervalSince1970: seen) : nil
    }

    static func record(_ date: Date, repoDir: String) {
        UserDefaults.standard.set(date.timeIntervalSince1970, forKey: key(repoDir))
    }

    private static func key(_ repoDir: String) -> String {
        "pawSyncLastChecked:\(repoDir)"
    }
}

/// Sending and checking for changes: the pill, the sync sheet and the
/// screens after a sync. The core does the sync, in its own thread, on one
/// card-signed login; this keeps what the screens show. It never starts the
/// network or the card without a tap, but for one rule: what a background
/// check left for the person is brought up with the sign-in when paw comes
/// forward (`checkWhatTheBackgroundLeft`). The PIN goes from the sheet's
/// field straight to the core task. What it brings in is announced on the
/// live store; what the screens should show next is asked through
/// `requests`.
@MainActor @Observable
final class SyncModel {
    let sync: StoreSync
    let address: String

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
    /// The GitHub sign-in, which checks for changes without the key when
    /// it is for this store's repository.
    let gitHub: GitHubAccess?
    /// Said in the key sheet when a check fell back to the key.
    private(set) var keyPathNote: String?
    /// The last background check (`BackgroundRefresh`), for the status page.
    private(set) var backgroundRun: BackgroundRun?

    /// The unpushed queue.
    @ObservationIgnored private let queue: StoreEdits
    @ObservationIgnored private let live: LiveStore
    @ObservationIgnored private let requests: Signal<ScreenRequest>
    @ObservationIgnored private let reader: KeyReader
    @ObservationIgnored private let copies: CopyCache?
    /// The key task has had its touch: "Connecting" is over.
    @ObservationIgnored private var signedIn = false
    /// Where to go once the sheet has gone.
    @ObservationIgnored private var pendingRoute: Route?
    @ObservationIgnored private var pendingSettings = false
    /// What the last re-encrypt left, and why (only what it found trying).
    @ObservationIgnored private var lastLeft: [LeftHeldBack] = []
    /// The status page is up, or comes up when the key sheet closes.
    private var statusUp = false
    /// What the key sheet's task is.
    private var keyKind: SyncKind = .refresh
    @ObservationIgnored private let sheetId = UUID()
    private let workQueue = DispatchQueue(label: "paw-sync", qos: .userInitiated)

    init(sync: StoreSync, queue: StoreEdits, live: LiveStore, requests: Signal<ScreenRequest>, reader: KeyReader,
         copies: CopyCache?, gitHub: GitHubAccess?) {
        self.sync = sync
        self.queue = queue
        self.live = live
        self.requests = requests
        self.reader = reader
        self.copies = copies
        self.gitHub = gitHub
        keyTask = KeyTaskDriver(reader: reader)
        address = sync.address()
        lastChecked = LastChecked.read(repoDir: live.repoDir)
        recomputeLocal()
        backgroundRun = BackgroundRefresh.lastRun()
        live.changed.subscribe(self) { model, changed in
            model.storeChanged(changed)
        }
    }

    var host: String {
        storeAddressHost(address: address)
    }

    private var unpushed: Int {
        queue.unpushed.count
    }

    /// The sync sheet while it is up: the key sheet while the key's task
    /// has it, else the status page.
    var sheet: SyncSheet? {
        if keyTask.isShown {
            return SyncSheet(id: sheetId, mode: .key(kind: keyKind))
        }
        return statusUp ? SyncSheet(id: sheetId, mode: .status) : nil
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
            return .paused("\(changeCount(heldBack.count)) held back \u{00B7} Review")
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
        let base = kind.working(unpushed: unpushed)
        guard let progress, progress.total > 0 else { return base }
        return "\(base) \(progress.done) of \(progress.total)"
    }

    /// A tap on the pill: the status sheet, or with the disc, straight to
    /// the key sheet that updates the store (design.md 2.3).
    func pillTapped() {
        let state = pillState(unpushed: unpushed)
        if state == .needsKey || state == .notChecked {
            check()
        } else {
            showStatus()
        }
    }

    func showStatus() {
        guard sheet == nil else { return }
        recomputeLocal()
        statusUp = true
    }

    /// Whether "Send" is offered: never for a change that is held back, and
    /// not while the remote needs looking at first.
    func canSend(unpushed: Int) -> Bool {
        unpushed > 0 && heldBack.isEmpty && replaced == nil && refused.isEmpty
    }

    /// The local state again, without the network or the key.
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

    /// The store changed, here or by a sync: what is held back or refused
    /// is read again, and what a sync brought in is said on Home.
    private func storeChanged(_ changed: StoreChanged) {
        switch changed.change {
        case .synced(let report?, let replaced):
            summarise(report, origin: replaced ? .replaced : .sync, store: changed.store)
        case .removed:
            return
        case .wrote, .recipientsAccepted, .synced(nil, _), .reencrypted:
            break
        }
        recomputeLocal()
    }

    /// "Re-encrypt 3 changes": the key sheet, then the core's task that
    /// opens each held-back change with the key (a touch each) and encrypts
    /// it for its folder's new list. Only ever started by a tap.
    func startReencrypt() {
        guard reencryptReady > 0 else { return }
        start(.reencrypt)
    }

    // MARK: Running a send or a check

    /// "Send N changes" or "Check for changes": the key sheet, then the
    /// core's sign-in task.
    func start(_ kind: SyncKind) {
        guard running == nil, !accepting else { return }
        keyKind = kind
        // The status page, if up, turns into the key sheet.
        statusUp = false
        keyTask.start(job(kind))
    }

    /// "Try again" on the status page, after a failure.
    func retryFailed() {
        guard let failure else { return }
        if failure.during == .refresh {
            check()
        } else if failure.during == .reencrypt {
            startReencrypt()
        } else {
            start(failure.during)
        }
    }

    /// Cancel, or the sheet swiped away. A command waiting for a touch
    /// cannot be stopped; the core throws its answer away. The queue is
    /// left as it was.
    func cancelSheet() {
        if keyTask.isShown {
            keyTask.cancel()
            if running != nil {
                failure = SyncFailure.cancelled(during: keyKind, unpushed: unpushed)
            }
            running = nil
        }
        statusUp = false
        keyPathNote = nil
    }

    /// The sheet went away: go where it said to.
    func sheetDismissed() {
        if let route = pendingRoute {
            pendingRoute = nil
            requests.send(.show(route))
        }
        if pendingSettings {
            pendingSettings = false
            requests.send(.settings)
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
        if kind != .reencrypt, !signedIn, progress == nil {
            return "Connecting to \(host)\u{2026}"
        }
        return kind.working(unpushed: unpushed)
    }

    /// The task failed for a reason that is not the key's: the sheet turns
    /// into the status page, with the failure said inline.
    private func syncFailed(_ error: SyncError, kind: SyncKind, unpushed: Int) {
        running = nil
        failure = SyncFailure(error, during: kind, unpushed: unpushed, host: host)
        recomputeLocal()
        statusUp = true
    }

    /// The core's sync went through. The sheet shows Done for a second and
    /// closes, unless what came back needs looking at (replaced history,
    /// held-back changes): then it turns into the status page that says so.
    private func finished(_ kind: SyncKind, outcome: KeyOutcome) {
        running = nil
        let needsLook = switch outcome {
        case .reencrypted(let report): reencrypted(report)
        case .synced(let report): absorb(report, kind: kind)
        default: kind == .reencrypt ? reencrypted(nil) : absorb(nil, kind: kind)
        }
        if needsLook {
            statusUp = true
        }
    }

    /// What a sync that went through brought back, said on the pill, the
    /// status page and the card on Home. `true` if it needs looking at.
    private func absorb(_ report: SyncReport?, kind: SyncKind) -> Bool {
        lastLeft = []
        let now = Date()
        lastChecked = now
        LastChecked.record(now, repoDir: live.repoDir)
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
                    requests.send(.toast("Sent"))
                }
            }
        }
        live.announce(.synced(report?.integration, replaced: false))
        refused = (try? sync.refusedRemote()) ?? []
        return needsLook
    }

    /// A re-encrypt that ran to the end (its end is announced with every
    /// other re-encrypt's, in `keyTaskEnded`). Always back to the status
    /// page after: what is left held back is said there, and Send is there
    /// once none is.
    private func reencrypted(_ report: ReencryptReport?) -> Bool {
        lastLeft = report?.left ?? []
        if let report, report.reencrypted > 0 {
            requests.send(.toast("Re-encrypted. Not sent yet."))
        }
        return true
    }

    // MARK: After a background check

    /// paw is on screen again, or a background check ended while it was:
    /// what background checks brought in shows in the lists, and what they
    /// left for the person comes up as a check from the pill would.
    func cameForward() {
        backgroundRun = BackgroundRefresh.lastRun()
        if let seen = LastChecked.read(repoDir: live.repoDir) {
            lastChecked = seen
        }
        guard running == nil, !accepting else { return }
        let pending = BackgroundRefresh.takePending()
        if pending.changed {
            live.announce(.synced(nil, replaced: false))
        } else {
            recomputeLocal()
        }
        if pending.needsLook {
            checkWhatTheBackgroundLeft()
        }
    }

    /// The one time paw starts the network without a tap: a background
    /// check found something only the person can decide (two versions
    /// kept, a delete that didn't happen, a changed list of keys). With the
    /// sign-in, no key, it is checked for again, so it comes up as a check
    /// from the pill would bring it up. Without the sign-in it waits for
    /// a tap.
    private func checkWhatTheBackgroundLeft() {
        if checksWithSignIn {
            checkWithSignIn()
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
    func check() {
        if checksWithSignIn {
            checkWithSignIn()
        } else {
            start(.refresh)
        }
    }

    /// Fetches over HTTPS with the sign-in and integrates as the key's
    /// check does, with no key sheet. Sends nothing. If the sign-in has
    /// ended, the check goes on with the key, and the key sheet says why.
    private func checkWithSignIn() {
        guard running == nil, !accepting, let gitHub else { return }
        failure = nil
        running = .refresh
        progress = nil
        keyPathNote = nil
        let signIn = SignInCheck(core: gitHub.core)
        let time = BackgroundTime.begin("Sync")
        workQueue.async { [weak self] in
            let checked = Handoff(value: signIn.run())
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    time.end()
                    self?.checkedWithSignIn(checked.value)
                }
            }
        }
    }

    private func checkedWithSignIn(_ result: Result<SyncReport, Error>) {
        running = nil
        gitHub?.reload()
        switch result {
        case .success(let report):
            if absorb(report, kind: .refresh) {
                recomputeLocal()
                // Over the key sheet if one is up, and staying once it closes.
                statusUp = true
            }
        case .failure(let error):
            if case .Remote(let failure)? = error as? SyncError, signInEnded(failure) {
                start(.refresh)
                keyPathNote = "Your GitHub sign-in ended, so this check uses your key."
                return
            }
            if let error = error as? SyncError {
                failure = SyncFailure(error, during: .refresh, unpushed: unpushed, host: "github.com")
            } else {
                failure = SyncFailure.signInCheckFailed(error, unpushed: unpushed)
            }
            recomputeLocal()
        }
    }

    /// The card on Home: what a sync brought in, and which Face ID copies
    /// it ended because their entry's text changed (the core finds them,
    /// off the main thread).
    private func summarise(_ report: IntegrateReport, origin: SyncSummary.Origin, store: PassStore) {
        let copies = copies
        workQueue.async { [weak self] in
            var summary = SyncSummary(origin: origin, report: report)
            if let copies {
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
                    if !handoff.value.isEmpty || !handoff.value.endedCopies.isEmpty {
                        self.summary = handoff.value
                    }
                }
            }
        }
    }

    // MARK: Replaced history

    /// Takes the git remote's new history, keeping this iPhone's waiting
    /// changes (replayed onto it) or discarding them. Local work only: no
    /// key, no network.
    func acceptReplaced(keepChanges: Bool) {
        guard replaced != nil, running == nil, !accepting else { return }
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
            live.announce(.synced(report, replaced: true))
            requests.send(.leave(.sync(.replaced)))
            requests.send(.toast(keepChanges ? "Using the new store. Your changes are kept." : "Using the new store."))
        case .failure(let error):
            acceptProblem = describe(error)
        }
    }

    /// "Not now" on the replaced screen: sync stays paused.
    func notNow() {
        requests.send(.leave(.sync(.replaced)))
    }
}

extension SyncModel {
    /// A send, a check or a re-encrypt with the key.
    private func job(_ kind: SyncKind) -> KeyJob<SyncError> {
        let sync = sync
        let port = reader.port
        return KeyJob(
            begin: { listener in
                let task: KeyTask = switch kind {
                case .send: sync.sendWithKey(port: port, listener: listener)
                case .refresh: sync.refreshWithKey(port: port, listener: listener)
                case .reencrypt: sync.reencryptWithKey(port: port, listener: listener)
                }
                BackgroundTime.hold("Sync", while: task)
                return task
            },
            words: { [weak self] in
                syncKeyWords(during: kind, unpushed: self?.unpushed ?? 0)
            },
            working: { [weak self] progress in
                self?.working(kind, progress) ?? ("", nil)
            },
            ended: { [weak self] end in
                self?.ended(end, kind: kind)
            },
            letGo: { [weak self] _, _ in
                // A re-encrypt may have rewritten some changes, the last of
                // them after its Cancel.
                if kind == .reencrypt {
                    self?.live.announce(.reencrypted)
                }
            },
            starting: { [weak self] in
                self?.taskStarting(kind)
            },
            stageShown: { [weak self] stage in
                if stage == .touch {
                    self?.signedIn = true
                }
            })
    }

    /// The Working stage, and the pill's count while the task runs.
    private func working(_ kind: SyncKind, _ progress: TaskProgress?) -> (message: String, progress: String?) {
        if kind == .reencrypt {
            // The key's answer clears it; the count goes on.
            if let progress { self.progress = progress }
            return (workingMessage(kind, unpushed: unpushed), nil)
        }
        self.progress = progress
        return (workingMessage(kind, unpushed: unpushed), progress.map(\.counted))
    }

    /// A send, check or re-encrypt starts, or starts again on Try again.
    private func taskStarting(_ kind: SyncKind) {
        failure = nil
        running = kind
        progress = nil
        signedIn = false
        if kind == .reencrypt {
            lastLeft = []
        }
    }

    private func ended(_ end: KeyTaskEnd<SyncError>, kind: SyncKind) -> KeySheetStage? {
        // A re-encrypt may have rewritten some changes, however it ended.
        defer {
            if kind == .reencrypt {
                live.announce(.reencrypted)
            }
        }
        switch end {
        case .done(let outcome):
            finished(kind, outcome: outcome)
            return .done
        case .stepFailed(let error):
            syncFailed(error, kind: kind, unpushed: unpushed)
            return nil
        case .cancelled:
            cancelSheet()
            return nil
        case .failed:
            running = nil
            return end.standardStage(syncKeyWords(during: kind, unpushed: unpushed))
        }
    }
}
