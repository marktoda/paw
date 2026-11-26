import Foundation
import Observation
import SwiftUI

/// The unpushed queue, sending and checking for changes: the pill, the
/// sync sheet and the screens after a sync. It never starts the network or
/// the card without a tap, but for one rule: what a background check left
/// for the person is brought up with the sign-in when paw comes forward
/// (`checkWhatTheBackgroundLeft`). The PIN goes from the sheet's field
/// straight to the core task.
@MainActor @Observable
final class SyncModel {
    let sync: StoreSync
    let address: String
    let queue: ChangeQueue

    let keyTask: KeyTaskDriver
    private(set) var running: SyncKind? {
        didSet {
            if running == nil, oldValue != nil { recomputeLocal() }
        }
    }
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
    private(set) var accepting = false {
        didSet {
            if !accepting, oldValue { recomputeLocal() }
        }
    }
    var acceptProblem: String?
    /// The GitHub sign-in, which checks for changes without the key when
    /// it is for this store's repository.
    let gitHub: GitHubAccess?
    /// Said in the key sheet when a check fell back to the key.
    private(set) var keyPathNote: String?
    private(set) var backgroundRun: BackgroundRun?

    @ObservationIgnored private let live: LiveStore
    @ObservationIgnored private let requests: Signal<ScreenRequest>
    @ObservationIgnored private let keyPort: KeyPort
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
    private var keyKind: SyncKind = .refresh
    @ObservationIgnored private let sheetId = UUID()
    private let workQueue = DispatchQueue(label: "paw-sync", qos: .userInitiated)

    init(sync: StoreSync, live: LiveStore, requests: Signal<ScreenRequest>, keyTasks: KeyTaskDrivers,
         copies: CopyCache?, gitHub: GitHubAccess?) {
        self.sync = sync
        queue = ChangeQueue(sync: sync, live: live)
        self.live = live
        self.requests = requests
        keyPort = keyTasks.reader.port
        self.copies = copies
        self.gitHub = gitHub
        keyTask = keyTasks.make()
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

    var sheet: SyncSheet? {
        if keyTask.isShown {
            return SyncSheet(id: sheetId, mode: .key(kind: keyKind))
        }
        return statusUp ? SyncSheet(id: sheetId, mode: .status) : nil
    }

    var pillState: SyncPillState {
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
        let state = pillState
        if state == .needsKey || state == .notChecked {
            check()
        } else {
            showStatus()
        }
    }

    func showStatus() {
        guard sheet == nil else { return }
        statusUp = true
    }

    /// Whether "Send" is offered: never for a change that is held back, and
    /// not while the remote needs looking at first.
    var canSend: Bool {
        unpushed > 0 && heldBack.isEmpty && replaced == nil && refused.isEmpty
    }

    private func recomputeLocal() {
        // A list that can't be read now keeps what was read last, never
        // "nothing": the core checks both again before it sends.
        if let found = try? sync.heldBack() { heldBack = found }
        if let found = try? sync.refusedRemote() { refused = found }
        recomputeReencrypt()
    }

    private func recomputeReencrypt() {
        // Unreadable: nothing is offered to re-encrypt until it can be read.
        let check: ReencryptCheck? = heldBack.isEmpty ? nil : (try? sync.reencryptCheck())
        reencryptReady = Int(check?.ready ?? 0)
        let held = Set(heldBack.map(\.entry))
        var left: [String: LeftReason] = [:]
        for item in lastLeft + (check?.left ?? []) where held.contains(item.entry) {
            left[item.entry] = item.reason
        }
        reencryptLeft = left
    }

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

    func start(_ kind: SyncKind) {
        guard running == nil, !accepting else { return }
        let sync = sync
        let job: KeyJob<SyncError> = switch kind {
        case .send: syncJob(.send) { port, listener in sync.sendWithKey(port: port, listener: listener) }
        case .refresh: syncJob(.refresh) { port, listener in sync.refreshWithKey(port: port, listener: listener) }
        case .reencrypt: reencryptJob()
        }
        keyKind = kind
        // The status page, if up, turns into the key sheet.
        statusUp = false
        keyTask.start(job)
    }

    /// "Try again" on the status page, after a failure.
    func retryFailed() {
        guard let failure else { return }
        switch failure.during {
        case .refresh: check()
        case .send: start(.send)
        case .reencrypt: startReencrypt()
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

    func go(to route: Route) {
        pendingRoute = route
        cancelSheet()
    }

    func openSettings() {
        pendingSettings = true
        cancelSheet()
    }

    /// What a sync that went through brought back, said on the pill, the
    /// status page and the card on Home. `true` if it needs looking at.
    private func absorb(_ report: SyncReport?, kind: SyncKind) -> Bool {
        lastLeft = []
        let now = Date()
        lastChecked = now
        LastChecked.record(now, repoDir: live.repoDir)
        var needsLook = false
        var waiting = false
        if let report {
            switch report.state {
            case .remoteReplaced(let found):
                replaced = found
                needsLook = true
            case .waiting:
                waiting = true
            case .upToDate:
                replaced = nil
                if kind == .send, report.pushed > 0 {
                    requests.send(.toast("Sent"))
                }
            }
        }
        live.announce(.synced(report?.integration, replaced: false))
        return needsLook || (waiting && kind == .send && !heldBack.isEmpty)
    }

    /// A re-encrypt that ran to the end. Always back to the status page
    /// after: what is left held back is said there, and Send is there once
    /// none is.
    private func reencrypted(_ outcome: KeyOutcome) -> Bool {
        guard case .reencrypted(let report) = outcome else {
            lastLeft = []
            return true
        }
        lastLeft = report.left
        if report.reencrypted > 0 {
            requests.send(.toast("Re-encrypted. Not sent yet."))
        }
        return true
    }

    /// paw is on screen again, or a background check ended while it was:
    /// what background checks brought in shows in the lists, and what they
    /// left for the person comes up as a check from the pill would.
    func cameForward() {
        let run = BackgroundRefresh.lastRun()
        // A check that ran fetched, even if it brought nothing in: what
        // is refused or held back may differ.
        let ranSince = run != backgroundRun
        backgroundRun = run
        if let seen = LastChecked.read(repoDir: live.repoDir) {
            lastChecked = seen
        }
        guard running == nil, !accepting else { return }
        let pending = BackgroundRefresh.takePending()
        if pending.changed || ranSince {
            live.announce(.synced(nil, replaced: false))
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

    /// The GitHub sign-in is for this store's repository: checking needs
    /// no key.
    var checksWithSignIn: Bool {
        gitHub?.covers(address: address) ?? false
    }

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
        let time = BackgroundTime.begin("Sync")
        gitHub.run({ try $0.refreshStore() }) { [weak self] checked in
            time.end()
            self?.checkedWithSignIn(checked)
        }
    }

    private func checkedWithSignIn(_ result: Result<SyncReport, Error>) {
        running = nil
        switch result {
        case .success(let report):
            if absorb(report, kind: .refresh) {
                // Over the key sheet if one is up, and staying once it closes.
                statusUp = true
            }
        case .failure(let error):
            if case .Fault(.remote(let failure))? = error as? SyncError, signInEnded(failure) {
                start(.refresh)
                keyPathNote = "Your GitHub sign-in ended, so this check uses your key."
                return
            }
            if let error = error as? SyncError {
                failure = SyncFailure(error, during: .refresh, unpushed: unpushed, host: "github.com")
            } else {
                failure = SyncFailure.signInCheckFailed(error, unpushed: unpushed)
            }
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
    /// Send, or check for changes, with the key.
    private func syncJob(_ kind: SyncKind,
                         _ begin: @escaping (KeyPort, KeyTaskListener) -> KeyTask) -> KeyJob<SyncError> {
        let port = keyPort
        var job = keyJob(kind) { listener in
            begin(port, listener)
        } working: { [weak self] progress in
            guard let self else { return ("", nil) }
            self.progress = progress
            let message = !signedIn && progress == nil
                ? "Connecting to \(host)\u{2026}" : kind.working(unpushed: unpushed)
            return (message, progress.map(\.counted))
        } done: { [weak self] outcome in
            guard let self else { return false }
            if case .synced(let report) = outcome {
                return absorb(report, kind: kind)
            }
            return absorb(nil, kind: kind)
        }
        job.stageShown = { [weak self] stage in
            if stage == .touch {
                self?.signedIn = true
            }
        }
        return job
    }

    /// "Re-encrypt": no network. Each held-back change is opened with the
    /// key (a touch each) and encrypted for its folder's new list.
    private func reencryptJob() -> KeyJob<SyncError> {
        let sync = sync
        let port = keyPort
        var job = keyJob(.reencrypt) { listener in
            sync.reencryptWithKey(port: port, listener: listener)
        } working: { [weak self] progress in
            guard let self else { return ("", nil) }
            // The key's answer clears it; the count goes on.
            if let progress { self.progress = progress }
            return (SyncKind.reencrypt.working(unpushed: unpushed), nil)
        } done: { [weak self] outcome in
            self?.reencrypted(outcome) ?? false
        }
        job.starting = { [weak self] in
            self?.taskStarting(.reencrypt)
            self?.lastLeft = []
        }
        // It may have rewritten some changes however it ended, the last of
        // them after its Cancel.
        job.afterEnd = { [weak self] in
            self?.live.announce(.reencrypted)
        }
        return job
    }

    /// What sending, checking and re-encrypting share: the sheet's words,
    /// background time, and how a failure or a cancel ends. `done` says
    /// whether what the task did needs looking at on the status page.
    private func keyJob(_ kind: SyncKind,
                        begin: @escaping (KeyTaskListener) -> KeyTask,
                        working: @escaping (TaskProgress?) -> (message: String, progress: String?),
                        done: @escaping (KeyOutcome) -> Bool) -> KeyJob<SyncError> {
        KeyJob(
            begin: begin,
            words: { [weak self] in
                syncKeyWords(during: kind, unpushed: self?.unpushed ?? 0)
            },
            working: working,
            ended: { [weak self] end in
                self?.ended(end, kind: kind, done: done) ?? .close
            },
            hold: { BackgroundTime.begin("Sync") },
            starting: { [weak self] in
                self?.taskStarting(kind)
            })
    }

    private func taskStarting(_ kind: SyncKind) {
        failure = nil
        running = kind
        progress = nil
        signedIn = false
    }

    private func ended(_ end: KeyTaskEnd<SyncError>, kind: SyncKind,
                       done: (KeyOutcome) -> Bool) -> KeySheetAfterEnd {
        switch end {
        case .done(let outcome):
            running = nil
            if done(outcome) {
                statusUp = true
            }
            return .standard
        case .stepFailed(let error):
            running = nil
            failure = SyncFailure(error, during: kind, unpushed: unpushed, host: host)
            statusUp = true
            return .close
        case .cancelled:
            cancelSheet()
            return .close
        case .failed:
            running = nil
            return .standard
        }
    }
}
