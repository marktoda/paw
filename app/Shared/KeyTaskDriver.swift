import Foundation
import Observation

/// Carries a value from a card, Face ID or work queue to the main actor once.
struct Handoff<Value>: @unchecked Sendable {
    let value: Value
}

/// Hands a core task's stage changes to the main queue. The core calls it
/// on its own threads, in order, and it must not block: it only queues.
final class MainQueueTaskListener: KeyTaskListener, @unchecked Sendable {
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

/// A workflow's own step after the key's part, which can fail on its own:
/// a sync, a setup step. `Never` for a workflow that is the key's part
/// alone (an open, the touch check).
protocol KeyStepError: Error {
    static func failure(in state: KeyTaskState) -> Self?
}

extension Never: KeyStepError {
    static func failure(in state: KeyTaskState) -> Never? {
        nil
    }
}

extension SyncError: KeyStepError {
    static func failure(in state: KeyTaskState) -> SyncError? {
        if case .syncFailed(let error) = state { return error }
        return nil
    }
}

extension SetupError: KeyStepError {
    static func failure(in state: KeyTaskState) -> SetupError? {
        if case .setupFailed(let error) = state { return error }
        return nil
    }
}

/// How a key task ended, for a workflow whose own step fails with `Step`.
enum KeyTaskEnd<Step: KeyStepError> {
    case done(KeyOutcome)
    case failed(KeyError)
    /// The key did its part and the workflow's own step after it failed.
    case stepFailed(Step)
    case cancelled

    /// `nil` while the task is still under way.
    init?(_ state: KeyTaskState) {
        switch state {
        case .done(let outcome):
            self = .done(outcome)
        case .cancelled:
            self = .cancelled
        case .failed(let error):
            self = .failed(error)
        case .syncFailed, .setupFailed:
            guard let failure = Step.failure(in: state) else {
                // The core gives each kind of task only the ends it can reach.
                assertionFailure("\(state) is not an end of this workflow's task")
                self = .cancelled
                return
            }
            self = .stepFailed(failure)
        case .plugIn, .pin, .busy, .touch, .working:
            return nil
        }
    }

    /// What the sheet shows for this end unless the workflow says
    /// otherwise: Done, the locked key, or the key's error with Try again.
    /// `nil` (close the sheet) for a cancel, and for the workflow's own
    /// failure, which it says on its own screen.
    func standardStage(_ words: KeyErrorWords.Context) -> KeySheetStage? {
        switch self {
        case .done:
            return .done
        case .failed(.PinBlocked):
            return .locked
        case .failed(let error):
            let said = words.words(error)
            return .failed(problem: said.problem, nextStep: said.nextStep)
        case .stepFailed, .cancelled:
            return nil
        }
    }
}

/// What the sheet does once the task it follows has ended.
enum KeySheetAfterEnd {
    /// The end's standard stage, in the job's words
    /// (`KeyTaskEnd.standardStage`). Done shows for a second, then the
    /// sheet closes by itself.
    case standard
    /// The sheet closes: the workflow shows what comes next itself.
    case close
}

/// Something a task keeps from its start to its real end, whether the sheet
/// still follows it or not: the app's background time (`BackgroundTime`).
@MainActor
protocol KeyTaskHold: AnyObject {
    func end()
}

/// One workflow's key task: what to start, how its sheet words it, and
/// what becomes of its end, whether the sheet still waits for it or not.
struct KeyJob<Step: KeyStepError> {
    /// Starts the core's task; Try again calls it again.
    var begin: (KeyTaskListener) -> KeyTask
    /// What the key's errors name and are for: on the failed stage, on the
    /// PIN stage when the PIN is asked again, and at Plug in when the key
    /// could not be read.
    var words: () -> KeyErrorWords.Context
    /// The Working stage's headline and progress line, for the task's
    /// progress.
    var working: (TaskProgress?) -> (message: String, progress: String?)
    /// The task the sheet follows ended; called once for each such task.
    var ended: (KeyTaskEnd<Step>) -> KeySheetAfterEnd = { _ in .standard }
    /// A task the sheet let go of (Cancel, Lock now, Try again) ended;
    /// called once for each such task. An open can still finish after its
    /// Cancel, in the last milliseconds after the card answered
    /// (`core/src/task/run.rs`): what it leaves is disposed of here.
    var letGo: (KeyTaskEnd<Step>) -> Void = { _ in }
    /// Any task ended, followed or let go of: called once for each, after
    /// `ended` or `letGo`.
    var afterEnd: () -> Void = {}
    /// Taken as each task starts, and ended once that task has ended,
    /// whether the sheet still follows it or not.
    var hold: () -> KeyTaskHold? = { nil }
    /// A task is about to start for the sheet, by a start or by Try again.
    var starting: () -> Void = {}
    /// The sheet moved on to a stage while the task runs.
    var stageShown: (KeySheetStage) -> Void = { _ in }
    /// The sheet closed, however it closed.
    var closed: () -> Void = {}
    /// The task starts, and starts again on Try again, only once every
    /// task the sheet let go of has ended (setup's download must be
    /// removed before the next one begins).
    var waitsForLetGoTasks = false
}

/// A `KeyJob` whatever its `Step`, as the driver keeps it. Only its ends
/// depend on `Step`, so they are read here, for the job's workflow.
private protocol AnyKeyJob {
    var begin: (KeyTaskListener) -> KeyTask { get }
    var words: () -> KeyErrorWords.Context { get }
    var working: (TaskProgress?) -> (message: String, progress: String?) { get }
    var afterEnd: () -> Void { get }
    var hold: () -> KeyTaskHold? { get }
    var starting: () -> Void { get }
    var stageShown: (KeySheetStage) -> Void { get }
    var closed: () -> Void { get }
    var waitsForLetGoTasks: Bool { get }
    /// The followed task ended in `state`: the stage to show, or `nil` to
    /// close.
    func stage(after state: KeyTaskState) -> KeySheetStage?
    /// A task the sheet let go of ended in `state`.
    func letGoEnded(_ state: KeyTaskState)
}

extension KeyJob: AnyKeyJob {
    fileprivate func stage(after state: KeyTaskState) -> KeySheetStage? {
        guard let end = KeyTaskEnd<Step>(state) else { return nil }
        switch ended(end) {
        case .standard: return end.standardStage(words())
        case .close: return nil
        }
    }

    fileprivate func letGoEnded(_ state: KeyTaskState) {
        if let end = KeyTaskEnd<Step>(state) { letGo(end) }
    }
}

/// Drives the core's key tasks for one key sheet. Whether a key sheet
/// is up, and what it shows, is said here alone. The PIN typed goes from
/// `pin` straight to the core and is never kept.
///
/// The sheet follows one task at a time. A task it lets go of (Cancel,
/// Try again, Lock now) is cancelled, and its end still reaches its
/// workflow, once.
@MainActor @Observable
final class KeyTaskDriver {
    /// What is typed in the sheet's PIN field. Handed to the core and
    /// cleared at once on Continue; cleared too whenever the sheet stops
    /// asking for the PIN.
    var pin = ""
    /// The sheet's run of tasks, kept across Try again; `nil` while no key
    /// sheet is up.
    private(set) var shown: UUID?
    /// Reads the key the task waits for at Plug in. A read that failed
    /// shows on that stage, with Try again. `nil` only until AutoFill has
    /// opened the store, before any task can start.
    var reader: KeyReader?

    private var mapped: KeySheetStage = .busy
    private var followed: UUID?
    @ObservationIgnored private var job: (any AnyKeyJob)?
    @ObservationIgnored private var startWaits = false
    @ObservationIgnored private var tasks: [UUID: Running] = [:]

    private struct Running {
        let task: KeyTask
        let job: any AnyKeyJob
        let hold: KeyTaskHold?
    }

    init(reader: KeyReader?) {
        self.reader = reader
    }

    var isShown: Bool {
        shown != nil
    }

    /// What the sheet shows. A key that could not be read shows as the
    /// failed stage, with Try again, while its task still waits at Plug in.
    var stage: KeySheetStage {
        guard let problem = readProblem, let job else { return mapped }
        let words = KeyErrorWords(problem, table: job.words().table)
        return .failed(problem: words.problem, nextStep: words.nextStep)
    }

    /// The followed task waits at Plug in and the key there could not be
    /// read: Try again reads it again rather than starting a new task.
    var readFailed: Bool {
        followed != nil && readProblem != nil
    }

    private var readProblem: KeyReadProblem? {
        guard case .plugIn = mapped else { return nil }
        return reader?.problem
    }

    var isFollowing: Bool {
        followed != nil
    }

    /// What the followed task is for, or the last one's once it ended.
    @ObservationIgnored private(set) var taskPurpose: KeyPurpose?

    /// Puts the sheet up for `job`'s task. A task the sheet followed until
    /// now is let go of.
    func start<Step>(_ job: KeyJob<Step>) {
        #if DEBUG && targetEnvironment(simulator)
        let adapted = reader?.simulated?.adapted(job) ?? job
        #else
        let adapted = job
        #endif
        letGoOfFollowed()
        shown = UUID()
        self.job = adapted
        show(.busy)
        launchOrWait(adapted)
        // A reader is there but its key was never read (or the read
        // failed): try again now rather than wait for a replug.
        reader?.readIfNeeded()
    }

    /// Continue on a PIN stage. The PIN goes straight to the core, which
    /// refuses one of a length no card takes without using a try.
    func continueWithPin() {
        guard let id = followed, let running = tasks[id], !pin.isEmpty, mapped.asksForPin else { return }
        running.task.supplyPin(pin: Data(pin.utf8))
        pin = ""
    }

    /// "Try again": a key that could not be read is read again, and the
    /// task waiting at Plug in goes on by itself once it is; otherwise a
    /// new task, which opens at the first stage still needed (Touch if
    /// the PIN is still held).
    func retry() {
        if readFailed {
            reader?.retry()
            show(.plugIn)
            return
        }
        guard shown != nil, let job else { return }
        letGoOfFollowed()
        show(.busy)
        launchOrWait(job)
    }

    /// Cancel, or the sheet swiped away: the sheet closes. Cancel always
    /// works. A card command already waiting for a touch cannot be
    /// stopped; the core throws its answer away.
    func cancel() {
        letGoOfFollowed()
        startWaits = false
        close()
    }

    /// Lets go of every task, followed or not: the request or the screen
    /// is going away, or Lock now.
    func cancelAll() {
        cancel()
        for (id, running) in tasks {
            running.task.cancel()
            changed(id)
        }
    }

    private func launchOrWait(_ job: any AnyKeyJob) {
        if job.waitsForLetGoTasks, !tasks.isEmpty {
            startWaits = true
        } else {
            launch()
        }
    }

    private func launch() {
        guard let job else { return }
        startWaits = false
        pin = ""
        job.starting()
        let id = UUID()
        let listener = MainQueueTaskListener { [weak self] in
            MainActor.assumeIsolated {
                self?.changed(id)
            }
        }
        let hold = job.hold()
        let task = job.begin(listener)
        tasks[id] = Running(task: task, job: job, hold: hold)
        taskPurpose = task.purpose()
        followed = id
        // The listener's word comes a main-queue turn later; whoever
        // started the job reads the sheet in this one.
        changed(id)
    }

    private func letGoOfFollowed() {
        guard let id = followed else { return }
        followed = nil
        guard let running = tasks[id] else { return }
        running.task.cancel()
        changed(id)
    }

    private func close() {
        guard shown != nil else { return }
        shown = nil
        pin = ""
        job?.closed()
    }

    /// A task changed state (or was just started or let go of): show it if
    /// the sheet follows it, hand its end over once it ends.
    private func changed(_ id: UUID) {
        guard let running = tasks[id] else { return }
        let state = running.task.state()
        guard id == followed else {
            if state.isFinal() {
                settle(id, running, state)
            }
            return
        }
        if case .failed(.KeyRemoved) = state, let reader, reader.present {
            // A transport error ended the session but the reader is still
            // there: read the key again, so Try again finds it.
            reader.read()
        }
        guard state.isFinal() else {
            show(stage(for: state, running.job))
            return
        }
        tasks[id] = nil
        followed = nil
        running.hold?.end()
        let run = shown
        let next = running.job.stage(after: state)
        running.job.afterEnd()
        // The workflow may have closed the sheet, or started again, as it heard the end.
        guard shown == run, run != nil else { return }
        guard let next else {
            close()
            return
        }
        show(next)
        if next == .done {
            closeAfterDone()
        }
    }

    /// A task the sheet let go of has ended.
    private func settle(_ id: UUID, _ running: Running, _ state: KeyTaskState) {
        tasks[id] = nil
        running.hold?.end()
        running.job.letGoEnded(state)
        running.job.afterEnd()
        if startWaits, tasks.isEmpty {
            launch()
        }
    }

    private func show(_ stage: KeySheetStage) {
        mapped = stage
        if !stage.asksForPin {
            pin = ""
        }
        job?.stageShown(stage)
    }

    private func closeAfterDone() {
        guard let run = shown else { return }
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard let self, self.shown == run, self.mapped == .done else { return }
            self.close()
        }
    }

    private func stage(for state: KeyTaskState, _ job: any AnyKeyJob) -> KeySheetStage {
        switch state {
        case .plugIn:
            return .plugIn
        case .pin(let triesLeft, let forgotten, let lastError):
            let tries = Int(triesLeft)
            switch lastError {
            case nil:
                return .pin(triesLeft: tries, forgotten: forgotten)
            case .WrongPin(let left)?:
                return .wrongPin(triesLeft: Int(left))
            case let error?:
                let words = job.words().words(error, pinAskedAgain: true)
                return .pinAgain(triesLeft: tries, problem: words.problem, nextStep: words.nextStep)
            }
        case .busy:
            return .busy
        case .touch:
            return .touch
        case .working(let progress):
            let working = job.working(progress)
            return .working(message: working.message, progress: working.progress)
        case .done, .failed, .syncFailed, .setupFailed, .cancelled:
            return mapped
        }
    }

    #if DEBUG && targetEnvironment(simulator)
    /// Key tasks not yet ended, and a start waiting for them, for the UI
    /// tests' "task ended" signal.
    var pendingTasks: Int {
        tasks.count + (startWaits ? 1 : 0)
    }
    #endif
}
