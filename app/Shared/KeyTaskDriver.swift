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

/// How a key task ended.
enum KeyTaskResult {
    case done(KeyOutcome)
    case failed(KeyError)
    /// The key did its part and the sync after it failed.
    case syncFailed(SyncError)
    /// The key did its part and the setup step after it failed.
    case setupFailed(SetupError)
    /// Stopped by Cancel.
    case cancelled

    /// `nil` while the task is still under way.
    init?(_ state: KeyTaskState) {
        switch state {
        case .done(let outcome): self = .done(outcome)
        case .failed(.Cancelled): self = .cancelled
        case .failed(let error): self = .failed(error)
        case .syncFailed(let error): self = .syncFailed(error)
        case .setupFailed(let error): self = .setupFailed(error)
        case .cancelled: self = .cancelled
        case .plugIn, .pin, .busy, .touch, .working: return nil
        }
    }
}

extension KeyTaskState {
    /// Done, failed or cancelled: nothing changes it any more.
    var isFinal: Bool {
        KeyTaskResult(self) != nil
    }
}

/// One task's end, as its owner hears it.
struct KeyTaskEnd {
    let result: KeyTaskResult
    /// What the task was for, as the core says.
    let purpose: KeyPurpose
    /// The driver's `generation` when the task started.
    let generation: Int
}

/// What differs between the workflows that run key tasks: what the sheet's
/// words name, and what happens when a task ends.
@MainActor
protocol KeyTaskOwner: AnyObject {
    /// What the sheet's key errors name and are for, on the failed stage
    /// and on the PIN stage when the PIN is asked again.
    var keyWords: KeyErrorWords.Context { get }
    /// The Working stage for the task's progress.
    func workingStage(_ progress: TaskProgress?) -> KeySheetStage
    /// The task the sheet follows ended; called once for each such task.
    /// Returns what the sheet shows now, or `nil` when the owner closed it.
    func keyTaskEnded(_ end: KeyTaskEnd) -> KeySheetStage?
    /// A task the sheet let go of (Cancel, Lock now, Try again) ended;
    /// called once for each such task. An open can still finish after its
    /// Cancel, in the last milliseconds after the card answered
    /// (`core/src/key/task.rs`).
    func letGoTaskEnded(_ end: KeyTaskEnd)
    /// A task is about to start for the sheet, by a start or by Try again.
    func keyTaskStarting()
    /// The sheet moved on to `stage` while the task runs.
    func keyStageShown(_ stage: KeySheetStage)
    /// Done has been shown for a second and the sheet still shows it.
    func doneShown()
}

extension KeyTaskOwner {
    func letGoTaskEnded(_ end: KeyTaskEnd) {}
    func keyTaskStarting() {}
    func keyStageShown(_ stage: KeySheetStage) {}
    func doneShown() {}

    /// What the sheet shows for `result` unless the workflow says
    /// otherwise: Done, the locked key, or the key's error with Try again.
    /// `nil` (close the sheet) for a cancel, and for a sync or setup
    /// failure, which the workflow says on its own screen.
    func standardStage(_ result: KeyTaskResult) -> KeySheetStage? {
        switch result {
        case .done:
            return .done
        case .failed(.PinBlocked):
            return .locked
        case .failed(let error):
            let words = keyWords.words(error)
            return .failed(problem: words.problem, nextStep: words.nextStep)
        case .syncFailed, .setupFailed, .cancelled:
            return nil
        }
    }
}

/// Drives the core's key tasks for one key sheet: starts a task with its
/// listener, shows its state as the sheet's stage, hands the PIN over,
/// Try again and Cancel. The PIN typed goes from `pin` straight to the
/// core and is never kept.
///
/// The sheet follows one task at a time. A task it lets go of (Cancel,
/// Try again) is cancelled, and its end still reaches the owner, once.
@MainActor @Observable
final class KeyTaskDriver {
    /// The state of the task started last, as the core last gave it, also
    /// once the sheet let go of it: `cancelled` from the moment of Cancel.
    private(set) var state: KeyTaskState?
    /// What is typed in the sheet's PIN field. Handed to the core and
    /// cleared at once on Continue; cleared too whenever the sheet stops
    /// asking for the PIN.
    var pin = ""
    /// Key tasks not yet ended, and a start waiting for them, for the UI
    /// tests' "task ended" signal.
    private(set) var pendingTasks = 0
    /// Reads the key the task waits for at Plug in. A read that failed
    /// shows on that stage, with Try again.
    var reader: KeyReader?
    /// Lock now moves it on: an end says which generation its task
    /// started in.
    @ObservationIgnored var generation = 0
    @ObservationIgnored weak var owner: (any KeyTaskOwner)?
    #if DEBUG && targetEnvironment(simulator)
    /// The next followed task that finishes ends the sheet as if Cancel had
    /// been tapped just as the card answered (past the point where the core
    /// can stop it): the owner hears a cancel, then the task's own end as a
    /// late one. The simulated key cannot be cancelled that late by itself.
    @ObservationIgnored var simulatesLateCancelNext = false
    #endif

    private var mapped: KeySheetStage = .busy
    private var followed: UUID?
    @ObservationIgnored private var latest: UUID?
    /// Each sheet's run of tasks, kept across Try again: Done closes the
    /// sheet only if it is still the same run.
    @ObservationIgnored private var run: UUID?
    @ObservationIgnored private var begin: ((KeyTaskListener) -> KeyTask)?
    @ObservationIgnored private var purpose: KeyPurpose?
    @ObservationIgnored private let pausesOnDone: Bool
    @ObservationIgnored private let waitsForLetGoTasks: Bool
    @ObservationIgnored private var startWaits = false {
        didSet { countTasks() }
    }
    @ObservationIgnored private var tasks: [UUID: Running] = [:] {
        didSet { countTasks() }
    }

    private struct Running {
        let task: KeyTask
        let generation: Int
    }

    /// `pausesOnDone`: Done shows for a second before the owner hears
    /// `doneShown`. `waitsForLetGoTasks`: a start waits until every task
    /// the sheet let go of has ended (setup's download must be removed
    /// before the next one begins).
    init(reader: KeyReader? = nil, pausesOnDone: Bool = true, waitsForLetGoTasks: Bool = false) {
        self.reader = reader
        self.pausesOnDone = pausesOnDone
        self.waitsForLetGoTasks = waitsForLetGoTasks
    }

    /// What the key sheet shows.
    var stage: KeySheetStage {
        if case .plugIn = mapped, let problem = reader?.problem {
            let words = KeyErrorWords(problem, shownIn: owner?.keyWords.shownIn ?? .app)
            return .failed(problem: words.problem, nextStep: words.nextStep)
        }
        return mapped
    }

    /// The task waits at Plug in and the key could not be read.
    var readProblem: KeyReadProblem? {
        if case .plugIn = mapped { return reader?.problem }
        return nil
    }

    /// A task is under way for the sheet.
    var isFollowing: Bool {
        followed != nil
    }

    /// What the followed task is for, or the last one's once it ended.
    var taskPurpose: KeyPurpose? {
        purpose
    }

    /// Starts a task for the sheet with `begin`, which Try again calls
    /// again. A task the sheet followed until now is let go of.
    func start(_ begin: @escaping (KeyTaskListener) -> KeyTask) {
        letGoOfFollowed()
        run = UUID()
        self.begin = begin
        latest = nil
        state = nil
        show(.busy)
        if waitsForLetGoTasks, !tasks.isEmpty {
            startWaits = true
        } else {
            launch()
        }
        // A reader is there but its key was never read (or the read
        // failed): try again now rather than wait for a replug.
        reader?.readIfNeeded()
    }

    /// Continue on a PIN stage. The PIN goes straight to the core, which
    /// refuses one that is not 6 to 127 characters without using a try.
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
        if let id = followed, case .plugIn? = tasks[id]?.task.state() {
            reader?.retry()
            show(.plugIn)
            return
        }
        guard begin != nil else { return }
        letGoOfFollowed()
        show(.busy)
        launch()
    }

    /// Cancel always works. A card command already waiting for a touch
    /// cannot be stopped; the core throws its answer away.
    func cancel() {
        letGoOfFollowed()
        run = nil
        startWaits = false
        pin = ""
    }

    /// Lets go of every task, followed or not: the request or the screen
    /// is going away.
    func cancelAll() {
        cancel()
        for (id, running) in tasks {
            running.task.cancel()
            changed(id)
        }
    }

    private func launch() {
        guard let begin else { return }
        startWaits = false
        pin = ""
        owner?.keyTaskStarting()
        let id = UUID()
        let listener = MainQueueTaskListener { [weak self] in
            MainActor.assumeIsolated {
                self?.changed(id)
            }
        }
        let task = begin(listener)
        tasks[id] = Running(task: task, generation: generation)
        purpose = task.purpose()
        followed = id
        latest = id
        // A task can start past Plug in; listeners hear only changes.
        changed(id)
    }

    private func letGoOfFollowed() {
        guard let id = followed else { return }
        followed = nil
        guard let running = tasks[id] else { return }
        running.task.cancel()
        changed(id)
    }

    /// A task changed state (or was just started or let go of): show it if
    /// the sheet follows it, hand its end over once it ends.
    private func changed(_ id: UUID) {
        guard let running = tasks[id] else { return }
        let state = running.task.state()
        let result = KeyTaskResult(state)
        if id == latest {
            self.state = state
        }
        guard id == followed else {
            if let result {
                settle(id, running, result)
            }
            return
        }
        #if DEBUG && targetEnvironment(simulator)
        if simulatesLateCancelNext, let result, case .done = result {
            simulatesLateCancelNext = false
            followed = nil
            run = nil
            pin = ""
            _ = owner?.keyTaskEnded(KeyTaskEnd(result: .cancelled, purpose: running.task.purpose(), generation: running.generation))
            settle(id, running, result)
            return
        }
        #endif
        if case .failed(.KeyRemoved) = state, reader?.present == true {
            // A transport error ended the session but the reader is still
            // there: read the key again, so Try again finds it.
            reader?.read()
        }
        guard let result else {
            show(stage(for: state))
            return
        }
        tasks[id] = nil
        followed = nil
        let end = KeyTaskEnd(result: result, purpose: running.task.purpose(), generation: running.generation)
        guard let shown = owner?.keyTaskEnded(end) else {
            run = nil
            pin = ""
            return
        }
        show(shown)
        if shown == .done, pausesOnDone {
            closeAfterDone()
        }
    }

    /// A task the sheet let go of has ended.
    private func settle(_ id: UUID, _ running: Running, _ result: KeyTaskResult) {
        tasks[id] = nil
        owner?.letGoTaskEnded(KeyTaskEnd(result: result, purpose: running.task.purpose(), generation: running.generation))
        if startWaits, tasks.isEmpty {
            launch()
        }
    }

    private func show(_ stage: KeySheetStage) {
        mapped = stage
        if !stage.asksForPin {
            pin = ""
        }
        owner?.keyStageShown(stage)
    }

    /// Done shows for a second; then the owner closes the sheet, if it
    /// still shows this run's Done.
    private func closeAfterDone() {
        guard let shown = run else { return }
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard let self, self.run == shown, self.mapped == .done else { return }
            self.owner?.doneShown()
        }
    }

    private func stage(for state: KeyTaskState) -> KeySheetStage {
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
                let words = (owner?.keyWords ?? KeyErrorWords.Context(entry: "")).words(error, pinAskedAgain: true)
                return .pinAgain(triesLeft: tries, problem: words.problem, nextStep: words.nextStep)
            }
        case .busy:
            return .busy
        case .touch:
            return .touch
        case .working(let progress):
            return owner?.workingStage(progress) ?? .working(message: "", progress: nil)
        case .done, .failed, .syncFailed, .setupFailed, .cancelled:
            return mapped
        }
    }

    private func countTasks() {
        pendingTasks = tasks.count + (startWaits ? 1 : 0)
    }
}
