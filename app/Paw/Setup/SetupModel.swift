import Foundation
import Observation
import SwiftUI
#if !targetEnvironment(simulator)
import CryptoTokenKit
#endif

/// The screen setup shows. The core's state picks it; two of the core's
/// steps have two screens each (the key, then the address; the address,
/// then the host's answer).
enum SetupScreen: Equatable {
    case welcome
    case plugIn
    case yourKey(KeySummary)
    case address(notice: String?)
    case confirmHost(host: String, algorithm: String, fingerprint: String)
    case keyNotAccepted(host: String, publicKey: String)
    case download(address: String)
    case check(entries: UInt32)
    case done(opened: String, entries: UInt32)

    /// "Step 2 of 4", or `nil` before and after the steps.
    var step: Int? {
        switch self {
        case .welcome, .done: nil
        case .plugIn, .yourKey: 1
        case .address, .confirmHost, .keyNotAccepted: 2
        case .download: 3
        case .check: 4
        }
    }

    static let stepCount = 4
}

/// A problem shown inline on the screen it happened on: what happened,
/// then what to do. Never an alert.
struct SetupProblem: Equatable {
    enum Action: Equatable {
        /// Run the step that failed again.
        case retry
        /// Forget everything setup kept and begin again.
        case startOver
        /// "I expected this change": forget the host's changed key, then
        /// show the key it presents now for confirmation.
        case forgetHostKey
        /// Switch to the host's other endpoint (GitHub's port 443).
        case usePortFallback
    }

    /// A second way on, offered under the problem: a sentence and a button.
    struct Offer: Equatable {
        var text: String? = nil
        let title: String
        let identifier: String
        let action: Action
    }

    var kind: ErrorRow.Kind = .error
    let problem: String
    let nextStep: String
    /// Lines to read closely, such as fingerprints or the host's own words.
    var details: [String] = []
    var action: Action? = nil
    var actionTitle: String? = nil
    var offer: Offer? = nil
}

/// The key sheet while setup shows it.
struct SetupSheet: Identifiable {
    enum Purpose {
        case download, check
    }

    let id = UUID()
    let purpose: Purpose
    /// The core task the sheet follows now; "Try again" starts a new one.
    var taskID: UUID
    var stage: KeySheetStage
}

/// What the simulated host and key do for setup in the simulator. `nil` on
/// a device.
struct SetupSimulator {
    /// The simulated host's address, for the debug control on the address screen.
    let address: String
    /// The person trusted the host's key.
    let hostConfirmed: () -> Void
    /// A download failed for a reason that was not the key's.
    let downloadFailed: () -> Void
    /// "Add the key on the host": the host accepts the key from now on.
    let acceptKey: () -> Void
    /// Puts the simulated key in.
    let plugIn: () -> Void
    /// The simulated key is out at launch.
    let startsUnplugged: Bool
}

/// Wraps a value that crosses from a work queue to the main actor once.
private struct SetupHandoff<Value>: @unchecked Sendable {
    let value: Value
}

/// Reading the key failed before it was read: the session could not be
/// opened (design.md 2.11).
private struct SetupAttachFailure: Error {
    let error: Error
}

/// The key's PIN KDF is one paw cannot follow: refused before any PIN.
private struct SetupUnsupportedPinKdf: Error {
    let detail: String?
}

/// Hands a core task's stage changes to the main queue, in order.
private final class SetupTaskListener: KeyTaskListener, @unchecked Sendable {
    private let changed: @Sendable () -> Void

    init(changed: @escaping @Sendable () -> Void) {
        self.changed = changed
    }

    func stateChanged(state: KeyTaskState) {
        DispatchQueue.main.async(execute: changed)
    }
}

/// Drives the core's `Setup` for the setup screens. The core decides every
/// step; this keeps what the screens need besides: which of a step's two
/// screens is up, the problem to show, and the key sheet. The PIN goes
/// from the sheet's field straight to the core task, never kept here.
@MainActor @Observable
final class SetupModel {
    typealias Connect = @Sendable () throws -> CardConnection

    struct KeyNotAccepted: Equatable {
        let host: String
        let publicKey: String
    }

    /// What the done screen says about the store: its top-level folders and
    /// the lines of its top-level `.gpg-id`.
    struct StoreFacts: Equatable {
        let folders: Int
        let recipients: Int
    }

    let setup: Setup
    /// Where the store is cloned to.
    let repoPath: String
    /// Shared with Home afterwards, so the PIN typed here is still held.
    let keyPort: KeyPort
    let simulator: SetupSimulator?
    private let connect: Connect
    private let finished: () -> Void
    private let startOver: () -> Void

    private(set) var state: SetupState
    /// Past the welcome screen.
    private(set) var started: Bool
    /// Past "Your key", on to the address.
    private(set) var keyConfirmed = false
    private(set) var notAccepted: KeyNotAccepted?
    var problem: SetupProblem?
    /// The core's sentence about the address as typed.
    var addressProblem: String?
    /// A plain step under way, e.g. "Checking github.com…".
    private(set) var working: String?
    var addressText = ""
    var sheet: SetupSheet?
    /// What is typed in the sheet's PIN field: handed to the core task and
    /// cleared at once, and whenever the sheet moves on.
    var pinEntry = ""
    private(set) var readiness: SetupWriteReadiness?
    private(set) var storeFacts: StoreFacts?
    /// Hex fingerprint of the key's SSH key, once read this launch.
    private(set) var sshFingerprint: String?
    /// A key is in (a reader is present, or the simulated key is plugged).
    private(set) var keyPresent: Bool
    /// Why the key in could not be read (its session not opened), for the
    /// Plug in screen and its "Try again".
    private(set) var attachProblem: String?
    /// This iPhone has no smart card support at all (device only).
    private(set) var noSmartCards = false
    /// Core tasks not yet ended, for the UI tests' `sim-key-tasks`.
    private(set) var pendingTasks = 0
    /// A download was cancelled while the key waited for its touch: the
    /// key still waits. The simulator's download screen offers to time it
    /// out; on a device the key's own timeout ends it.
    private(set) var touchLeftWaiting = false

    @ObservationIgnored private var tasks: [UUID: KeyTask] = [:] {
        didSet { countTasks() }
    }
    /// The sheet is up but its task waits for a cancelled one to end: a
    /// download stopped while the key waited for a touch is still being
    /// removed, and the next may start only once it is.
    @ObservationIgnored private var waitingToStart = false {
        didSet { countTasks() }
    }
    /// The sheet's task waits at Plug in and opening the key's session
    /// failed: its "Try again" opens the session again.
    @ObservationIgnored private var sheetAwaitsAttach = false
    @ObservationIgnored private var attaching = false
    #if !targetEnvironment(simulator)
    @ObservationIgnored private var slotWatch: NSKeyValueObservation?
    #endif
    private let workQueue = DispatchQueue(label: "paw-setup", qos: .userInitiated)

    init(setup: Setup, repoPath: String, keyPort: KeyPort, connect: @escaping Connect, simulator: SetupSimulator?,
         finished: @escaping () -> Void, startOver: @escaping () -> Void) {
        self.setup = setup
        self.repoPath = repoPath
        self.keyPort = keyPort
        self.connect = connect
        self.simulator = simulator
        self.finished = finished
        self.startOver = startOver
        let state = setup.state()
        self.state = state
        started = state != .readKey
        keyPresent = keyPort.session() != nil || simulator.map { !$0.startsUnplugged } ?? false
        if case .checkHost(let address, _) = state {
            addressText = address
        }
        if case .done = state {
            learnAboutStore()
        }
        #if targetEnvironment(simulator)
        // Before the key is read, reading it opens the session, and shows
        // on Plug in if that fails.
        if keyPresent, state != .readKey { attachKey() }
        #else
        watchReaders()
        #endif
    }

    private func countTasks() {
        pendingTasks = tasks.count + (waitingToStart ? 1 : 0)
    }

    /// A key is in but has no session yet (a relaunch past the first step):
    /// open one, so the key sheet finds it. A failure shows on Plug in, or
    /// in the sheet if it waits at Plug in.
    private func attachKey() {
        guard keyPort.session() == nil, !attaching else { return }
        attaching = true
        let port = keyPort
        let connect = connect
        workQueue.async {
            let result: Result<Void, Error> = Result {
                _ = try port.pluggedIn(connection: connect())
            }
            let handoff = SetupHandoff(value: result)
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    self?.attached(handoff.value)
                }
            }
        }
    }

    private func attached(_ result: Result<Void, Error>) {
        attaching = false
        switch result {
        case .success:
            attachProblem = nil
        case .failure(let error):
            let words = Self.attachWords(error)
            attachProblem = words
            if var sheet, sheet.stage == .plugIn {
                sheet.stage = .failed(problem: "paw couldn't read your key", nextStep: words)
                self.sheet = sheet
                sheetAwaitsAttach = true
            }
        }
    }

    /// What went wrong opening the key's session, as a sentence or two.
    private static func attachWords(_ error: Error) -> String {
        if let error = error as? KeyError {
            let words = KeyErrorWords(error, entry: "your store", purpose: .check)
            return "\(words.problem). \(words.nextStep)"
        }
        if let error = error as? ConnectionError, case .Failed(let message) = error {
            return sentence(message)
        }
        return describe(error)
    }

    /// "Try again" under the attach problem on Plug in.
    func retryAttach() {
        attachProblem = nil
        if screen == .plugIn {
            readKey()
        } else {
            attachKey()
        }
    }

    /// The setup flow is back in the foreground: the session hears of it
    /// (it may drop a PIN held too long), and a key that is in but was not
    /// read is read again.
    func foregrounded() {
        keyPort.session()?.noteForegrounded()
        guard keyPresent, keyPort.session() == nil, working == nil else { return }
        if screen == .plugIn, sheet == nil {
            readKey()
        } else if started || state != .readKey {
            attachKey()
        }
    }

    var screen: SetupScreen {
        if let notAccepted {
            return .keyNotAccepted(host: notAccepted.host, publicKey: notAccepted.publicKey)
        }
        switch state {
        case .readKey:
            return started ? .plugIn : .welcome
        case .address(let key):
            return keyConfirmed ? .address(notice: nil) : .yourKey(key)
        case .checkHost(_, let notice):
            return .address(notice: notice)
        case .confirmHost(let host, let algorithm, let fingerprint):
            return .confirmHost(host: host, algorithm: algorithm, fingerprint: fingerprint)
        case .needPin(let address, _):
            return .download(address: address)
        case .testDecrypt(let entries, _):
            return .check(entries: entries)
        case .done(let opened, let entries):
            return .done(opened: opened, entries: entries)
        }
    }

    /// The host of the address set, for sentences: "github.com".
    var host: String {
        storeAddressHost(address: addressSet)
    }

    /// The address setup has, else the one typed.
    private var addressSet: String {
        switch state {
        case .checkHost(let set, _), .needPin(let set, _): set
        default: addressText
        }
    }

    /// What paw makes of the address as typed, for the line under the
    /// field; `nil` while it can't be read (the reason shows after Connect).
    var addressReading: AddressReading? {
        let typed = addressText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !typed.isEmpty else { return nil }
        return try? readStoreAddress(text: typed)
    }

    /// "Connects to github.com as git."
    static func readingLine(_ reading: AddressReading) -> String {
        let port = reading.port == 22 ? "" : " on port \(reading.port)"
        var line = "Connects to \(reading.host) as \(reading.user)\(port)."
        if reading.hostKeyBundled {
            line += " Its fingerprint is built in."
        }
        return line
    }

    // MARK: Moving through the steps

    /// "Set up" on the welcome screen: on to the key.
    func begin() {
        problem = nil
        started = true
        readKey()
    }

    /// Reads the plugged-in key from its session: no PIN, no touch.
    func readKey() {
        guard keyPresent, working == nil else { return }
        problem = nil
        attachProblem = nil
        working = "Reading your key\u{2026}"
        let port = keyPort
        let connect = connect
        let setup = setup
        workQueue.async {
            let result: Result<(SetupState, String?), Error> = Result {
                let session: KeySession
                if let held = port.session() {
                    session = held
                } else {
                    do {
                        session = try port.pluggedIn(connection: connect())
                    } catch {
                        throw SetupAttachFailure(error: error)
                    }
                }
                // Refused before any PIN: paw could not send this key its PIN.
                let kdf = session.pinKdf()
                guard kdf.supported else {
                    throw SetupUnsupportedPinKdf(detail: kdf.detail)
                }
                let info = session.info()
                do {
                    return (try setup.readKeyFromSession(session: session), info.authenticationFingerprint)
                } catch SetupError.UnusableKey {
                    // Worded by what the key lacks, as the key sheet words it.
                    if info.decryptionFingerprint == nil { throw KeyError.NoDecryptionKey }
                    if info.authenticationFingerprint == nil { throw KeyError.NoAuthenticationKey }
                    throw SetupError.UnusableKey(message: "it can't be used for setup")
                }
            }
            let handoff = SetupHandoff(value: result)
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    self?.keyRead(handoff.value)
                }
            }
        }
    }

    private func keyRead(_ result: Result<(SetupState, String?), Error>) {
        working = nil
        switch result {
        case .success(let (state, fingerprint)):
            self.state = state
            sshFingerprint = fingerprint
            keyConfirmed = false
        case .failure(let failure as SetupAttachFailure):
            state = setup.state()
            attachProblem = Self.attachWords(failure.error)
        case .failure(let refusal as SetupUnsupportedPinKdf):
            state = setup.state()
            problem = SetupProblem(
                problem: "paw doesn't support this key's PIN setting yet",
                nextStep: "It has KDF turned on. No PIN try was used.",
                details: refusal.detail.map { [$0] } ?? [])
        case .failure(let error as KeyError):
            state = setup.state()
            let words = KeyErrorWords(error, entry: "your store", purpose: .check)
            problem = SetupProblem(problem: words.problem, nextStep: words.nextStep)
        case .failure(let error):
            state = setup.state()
            show(error, during: .readingKey)
        }
    }

    /// "Continue" under the key's card.
    func continueFromKey() {
        problem = nil
        keyConfirmed = true
    }

    /// "Connect": the core reads the address, then checks the host and asks
    /// whether it accepts the key. No PIN, no touch.
    func connectToHost() {
        addressProblem = nil
        problem = nil
        let typed = addressText
        let host = storeAddressHost(address: typed)
        run("Checking \(host.isEmpty ? "the address" : host)\u{2026}", during: .checkingHost) { setup in
            _ = try setup.setAddress(address: typed)
            return try setup.checkHost()
        }
    }

    /// "Trust this server" on the host's fingerprint.
    func trustHost(fingerprint: String) {
        problem = nil
        run("Checking \(host)\u{2026}", during: .checkingHost) { setup in
            try setup.confirmHost(fingerprint: fingerprint)
        } then: { [weak self] _ in
            self?.simulator?.hostConfirmed()
        }
    }

    /// "Check again" once the key was added on the host.
    func checkAgain() {
        problem = nil
        run("Checking \(host)\u{2026}", during: .checkingHost) { setup in
            try setup.checkHost()
        } then: { [weak self] _ in
            self?.notAccepted = nil
        }
    }

    /// "I expected this change" on a changed host key: forgets the key
    /// pinned for the host, then checks it again, which shows the key it
    /// presents now for confirmation.
    func forgetHostKey() {
        problem = nil
        run("Checking \(host)\u{2026}", during: .forgettingHostKey) { setup in
            _ = try setup.forgetHostKey()
            return try setup.checkHost()
        }
    }

    /// "Try port 443": the address becomes the host's other endpoint, and
    /// the address screen says so under the field.
    func usePortFallback() {
        problem = nil
        run(nil, during: .checkingHost) { setup in
            try setup.usePortFallback()
        } then: { [weak self] state in
            if case .checkHost(let address, _) = state {
                self?.addressText = address
            }
        }
    }

    /// Back, on every screen but the first.
    func back() {
        problem = nil
        addressProblem = nil
        if notAccepted != nil {
            notAccepted = nil
            return
        }
        switch screen {
        case .welcome, .done, .keyNotAccepted:
            return
        case .plugIn:
            started = false
        case .yourKey:
            run(nil, during: .goingBack) { setup in try setup.back() } then: { [weak self] _ in
                self?.started = false
            }
        case .address:
            if case .address = state {
                keyConfirmed = false
            } else {
                run(nil, during: .goingBack) { setup in try setup.back() }
            }
        case .confirmHost, .download, .check:
            run(nil, during: .goingBack) { setup in try setup.back() } then: { [weak self] state in
                if case .checkHost(let address, _) = state {
                    self?.addressText = address
                }
            }
        }
    }

    /// "Open my passwords" on the last screen.
    func openStore() {
        finished()
    }

    /// The inline problem's action.
    func performProblemAction() {
        guard let action = problem?.action else { return }
        problem = nil
        perform(action)
    }

    /// The button the inline problem offers besides its action.
    func performOffer() {
        guard let action = problem?.offer?.action else { return }
        problem = nil
        perform(action)
    }

    private func perform(_ action: SetupProblem.Action) {
        switch action {
        case .forgetHostKey:
            forgetHostKey()
        case .usePortFallback:
            usePortFallback()
        case .startOver:
            startOver()
        case .retry:
            switch screen {
            case .download: startDownload()
            case .check: startCheck()
            case .address: connectToHost()
            case .plugIn: readKey()
            case .keyNotAccepted: checkAgain()
            default: break
            }
        }
    }

    private enum Moment {
        case readingKey, checkingHost, downloading, checkingKey, goingBack, forgettingHostKey
    }

    /// Runs a plain core step off the main thread; `then` sees its new state.
    private func run(_ message: String?, during moment: Moment,
                     _ work: @escaping @Sendable (Setup) throws -> SetupState,
                     then: @escaping (SetupState) -> Void = { _ in }) {
        guard working == nil else { return }
        working = message ?? ""
        let setup = setup
        let time = BackgroundTime.begin("Setup")
        workQueue.async {
            let handoff = SetupHandoff(value: Result { try work(setup) })
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    time.end()
                    guard let self else { return }
                    self.working = nil
                    switch handoff.value {
                    case .success(let state):
                        self.state = state
                        then(state)
                    case .failure(let error):
                        self.state = setup.state()
                        self.show(error, during: moment)
                    }
                }
            }
        }
    }

    // MARK: The key sheet

    /// "Download": the core's sign-in task, PIN (unless held), connecting,
    /// touch, then the clone with its progress.
    func startDownload() {
        startSheet(.download)
    }

    /// "Check my key": opens one entry and wipes it.
    func startCheck() {
        startSheet(.check)
    }

    private func startSheet(_ purpose: SetupSheet.Purpose) {
        guard sheet == nil else { return }
        problem = nil
        guard tasks.isEmpty else {
            sheet = SetupSheet(purpose: purpose, taskID: UUID(), stage: .busy)
            waitingToStart = true
            return
        }
        let taskID = startTask(purpose)
        sheet = SetupSheet(purpose: purpose, taskID: taskID, stage: .busy)
        taskChanged(taskID)
    }

    /// The last task the sheet let go of has ended: start the one waiting.
    private func startWaitingTask() {
        guard waitingToStart, tasks.isEmpty, var sheet else { return }
        waitingToStart = false
        sheet.taskID = startTask(sheet.purpose)
        self.sheet = sheet
        taskChanged(sheet.taskID)
    }

    private func startTask(_ purpose: SetupSheet.Purpose) -> UUID {
        pinEntry = ""
        let id = UUID()
        let listener = SetupTaskListener { [weak self] in
            MainActor.assumeIsolated {
                self?.taskChanged(id)
            }
        }
        switch purpose {
        case .download: tasks[id] = setup.downloadWithKey(port: keyPort, listener: listener)
        case .check: tasks[id] = setup.testDecryptWithKey(port: keyPort, listener: listener)
        }
        BackgroundTime.hold("Setup", while: tasks[id])
        return id
    }

    func continueWithPin() {
        guard let sheet, let task = tasks[sheet.taskID], !pinEntry.isEmpty else { return }
        guard Self.asksForPin(sheet.stage) else { return }
        task.supplyPin(pin: Data(pinEntry.utf8))
        pinEntry = ""
    }

    /// "Try again" in the sheet: a new task, at the first stage still needed.
    func retrySheet() {
        guard var sheet else { return }
        if sheetAwaitsAttach {
            // The task still waits at Plug in: open the key's session again.
            sheetAwaitsAttach = false
            sheet.stage = .plugIn
            self.sheet = sheet
            attachKey()
            return
        }
        tasks[sheet.taskID]?.cancel()
        sheet.taskID = startTask(sheet.purpose)
        sheet.stage = .busy
        self.sheet = sheet
        taskChanged(sheet.taskID)
    }

    /// Cancel always works. A download stopped part way leaves nothing: the
    /// core removes it and setup is back at the download step.
    func cancelSheet() {
        if let sheet {
            if sheet.stage == .touch {
                // The key's command can't be interrupted: it still waits.
                touchLeftWaiting = true
            }
            tasks[sheet.taskID]?.cancel()
        }
        sheet = nil
        sheetAwaitsAttach = false
        waitingToStart = false
        pinEntry = ""
    }

    #if targetEnvironment(simulator)
    /// The debug control on Plug in and on the sheet's Plug in stage: as a
    /// reader appearing on a device does.
    func simulatedPlugIn() {
        simulator?.plugIn()
        keyPresent = true
        if screen == .plugIn, sheet == nil {
            readKey()
        } else {
            attachKey()
        }
    }

    /// The debug control on the download screen after a cancel during the
    /// touch: the key's wait runs out now.
    func simulatedTouchTimeout() {
        touchLeftWaiting = false
        KeyConnection.simulateTouchTimeout()
    }
    #endif

    private func taskChanged(_ id: UUID) {
        guard let task = tasks[id] else { return }
        let taskState = task.state()
        guard var sheet, sheet.taskID == id else {
            // A task the sheet let go of: once it ends, show where setup is.
            if taskState.isFinal {
                tasks[id] = nil
                state = setup.state()
                startWaitingTask()
            }
            return
        }
        if taskState.isFinal {
            tasks[id] = nil
        }
        sheetAwaitsAttach = false
        let purpose: KeyErrorWords.Purpose = sheet.purpose == .download ? .download : .check
        switch taskState {
        case .plugIn:
            sheet.stage = .plugIn
        case .pin(let triesLeft, let forgotten, let lastError):
            let tries = Int(triesLeft)
            switch lastError {
            case nil:
                sheet.stage = .pin(triesLeft: tries, forgotten: forgotten)
            case .WrongPin(let left)?:
                sheet.stage = .wrongPin(triesLeft: Int(left))
            case let error?:
                let words = KeyErrorWords(pinAskedAgain: error, entry: entryName(task), purpose: purpose)
                sheet.stage = .pinAgain(triesLeft: tries, problem: words.problem, nextStep: words.nextStep)
            }
        case .busy:
            sheet.stage = .busy
        case .touch:
            sheet.stage = .touch
            // A new command waits for a touch, so any cancelled one has ended.
            touchLeftWaiting = false
        case .working(let progress):
            // Re-read on every change: objects and bytes, when known.
            let transfer = sheet.purpose == .download ? setup.transferProgress() : nil
            let line = transfer.flatMap(Self.transferLine) ?? progress.map(Self.progressLine)
            sheet.stage = .working(
                message: workingMessage(sheet.purpose, started: progress != nil || transfer != nil),
                progress: line)
        case .done:
            sheet.stage = .done
            self.sheet = sheet
            pinEntry = ""
            succeeded(sheet)
            return
        case .failed(.Reader):
            self.sheet = nil
            pinEntry = ""
            stopped(sheet.purpose, because: nil)
            return
        case .setupFailed(let error):
            // Not the key's problem: the host, the repository or the store.
            self.sheet = nil
            pinEntry = ""
            stopped(sheet.purpose, because: error)
            return
        case .failed(.Cancelled), .cancelled, .syncFailed:
            self.sheet = nil
            pinEntry = ""
            state = setup.state()
            return
        case .failed(.PinBlocked):
            sheet.stage = .locked
        case .failed(let error):
            let words = KeyErrorWords(error, entry: entryName(task), purpose: purpose)
            sheet.stage = .failed(problem: words.problem, nextStep: words.nextStep)
        }
        if !Self.asksForPin(sheet.stage) {
            pinEntry = ""
        }
        self.sheet = sheet
    }

    private func entryName(_ task: KeyTask) -> String {
        if case .checkKey(let entry?) = task.purpose(), !entry.isEmpty {
            return entry.split(separator: "/").last.map(String.init) ?? entry
        }
        return "your store"
    }

    private func workingMessage(_ purpose: SetupSheet.Purpose, started: Bool) -> String {
        switch purpose {
        case .download:
            started ? "Downloading your store\u{2026}" : "Connecting to \(host)\u{2026}"
        case .check:
            "Checking your key\u{2026}"
        }
    }

    private static func progressLine(_ progress: TaskProgress) -> String {
        "\(Int(progress.done).formatted()) of \(Int(progress.total).formatted())"
    }

    /// "412 of 1,240 \u{00B7} 1.2 MB", once the host has said how many.
    private static func transferLine(_ transfer: TransferProgress) -> String? {
        guard transfer.totalObjects > 0 else { return nil }
        let bytes = ByteCountFormatter.string(
            fromByteCount: Int64(clamping: transfer.receivedBytes), countStyle: .file)
        return "\(Int(transfer.receivedObjects).formatted()) of \(Int(transfer.totalObjects).formatted()) \u{00B7} \(bytes)"
    }

    private static func asksForPin(_ stage: KeySheetStage) -> Bool {
        switch stage {
        case .pin, .wrongPin, .pinAgain: true
        default: false
        }
    }

    private func succeeded(_ sheet: SetupSheet) {
        state = setup.state()
        if case .done = state {
            learnAboutStore()
        }
        let shown = sheet.id
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard let self, self.sheet?.id == shown else { return }
            self.sheet = nil
        }
    }

    /// For the done screen: whether saving works, and what is in the store.
    private func learnAboutStore() {
        readiness = try? setup.writeReadiness()
        if let store = try? PassStore(dir: repoPath) {
            let recipients = (try? store.recipients(folder: "")) ?? []
            storeFacts = StoreFacts(folders: store.home().folders.count, recipients: recipients.count)
        }
    }

    /// The task ended without its step done: `error` says why, if it was
    /// the setup's.
    private func stopped(_ purpose: SetupSheet.Purpose, because error: SetupError?) {
        state = setup.state()
        if let error {
            let refused = purpose == .download ? setup.refusedRemote() : []
            if refused.isEmpty {
                show(error, during: purpose == .download ? .downloading : .checkingKey)
            } else {
                problem = SetupProblem(
                    problem: "Your store has names paw can't download",
                    nextStep: "Rename or remove them on a computer, then try again.",
                    details: refused.map { "\($0.path): \($0.reason)" },
                    action: .retry, actionTitle: "Try again")
            }
        }
        if purpose == .download {
            simulator?.downloadFailed()
        }
    }

    // MARK: Words

    private func show(_ error: Error, during moment: Moment) {
        guard let error = error as? SetupError else {
            problem = SetupProblem(
                problem: "Setup stopped", nextStep: "Try again.", details: [describe(error)], action: .retry,
                actionTitle: "Try again")
            return
        }
        // What the host said picks the words for a few failures that share
        // a case, "no store at that address" in particular.
        if [.checkingHost, .downloading, .forgettingHostKey].contains(moment),
           let remote = setup.lastRemoteMessage(), let words = Self.remoteWords(remote) {
            problem = words
            return
        }
        switch error {
        case .BadAddress(let message):
            addressProblem = message
        case .KeyNotAccepted(let host, let publicKey):
            notAccepted = KeyNotAccepted(host: host, publicKey: publicKey)
        case .HostKeyChanged(let host, let expected, let presented):
            let bundled = (try? readStoreAddress(text: addressSet))?.hostKeyBundled ?? false
            problem = Self.hostKeyChanged(host: host, expected: expected, presented: presented, bundled: bundled)
        case .HostUnreachable:
            var words = Self.words(error, during: moment)
            if let fallback = setup.portFallback() {
                words.offer = SetupProblem.Offer(
                    text: "Port \(fallback.fromPort) seems blocked. Try \(fallback.host) on port \(fallback.port)?",
                    title: "Try port \(fallback.port)", identifier: "use-port-fallback", action: .usePortFallback)
            }
            problem = words
        case .Failed(let message) where moment == .forgettingHostKey:
            // A host whose keys ship with paw: its key is never forgotten.
            problem = SetupProblem(
                problem: "paw kept this server's key", nextStep: Self.sentence(message),
                action: .startOver, actionTitle: "Start setup again")
        case .StoreBusy where moment == .forgettingHostKey:
            problem = SetupProblem(
                problem: "paw kept this server's key", nextStep: Self.sentence(storeBusyText),
                action: .startOver, actionTitle: "Start setup again")
        default:
            problem = Self.words(error, during: moment)
        }
    }

    /// The words for what the last host check or download found, when it
    /// has words of its own.
    private static func remoteWords(_ remote: RemoteMessage) -> SetupProblem? {
        switch remote.reason {
        case .repositoryNotFound:
            let repository = remote.repository.isEmpty ? "that repository" : remote.repository
            let host = remote.host.isEmpty ? "The server" : remote.host
            return SetupProblem(
                problem: "No store at that address",
                nextStep: "\(host) says \(repository) doesn't exist, or this key can't see it. Check the address "
                    + "and your account's access.",
                details: remote.said.map { ["\(host) said: \($0)"] } ?? [],
                action: .retry, actionTitle: "Try again")
        case .emptyRepository:
            return words(.EmptyRepository, during: .downloading)
        case .notAPassStore:
            return words(.NotAPassStore, during: .downloading)
        case .keyNotAccepted, .unreachable, .hostKeyChanged, .other:
            return nil
        }
    }

    /// A pinned host presented another key. For a host paw has no keys of
    /// its own for, the person may say they expected it.
    private static func hostKeyChanged(host: String, expected: [String], presented: String,
                                       bundled: Bool) -> SetupProblem {
        let warning = "This can mean someone is intercepting the connection. Nothing was sent."
        let advice: String
        if bundled {
            advice = "\(host)'s real keys are built into paw, so this isn't \(host). Try another network."
        } else {
            advice = "If the server's admin confirms the change, tap I expected this change."
        }
        var words = SetupProblem(
            problem: "\(host)'s identity changed",
            nextStep: "\(warning) \(advice)",
            details: expected.map { "Expected \($0)" } + ["It presented \(presented)"],
            action: .startOver, actionTitle: "Start setup again")
        if !bundled {
            words.offer = SetupProblem.Offer(
                title: "I expected this change", identifier: "host-key-expected", action: .forgetHostKey)
        }
        return words
    }

    /// The failure table of design.md 2.4, S3 and S4.
    private static func words(_ error: SetupError, during moment: Moment) -> SetupProblem {
        let downloading = moment == .downloading
        switch error {
        case .HostUnreachable(let host, let detail):
            if downloading {
                return SetupProblem(
                    problem: "The download was interrupted",
                    nextStep: "Nothing was kept. Check your connection and try again.",
                    details: [detail], action: .retry, actionTitle: "Try again")
            }
            return SetupProblem(
                problem: "Can't reach \(host)", nextStep: "Check your connection and try again.",
                details: [detail], action: .retry, actionTitle: "Try again")
        case .HostKeyChanged(let host, let expected, let presented):
            // `show` knows whether the host's keys ship with paw; without
            // that, nothing is offered but starting again.
            return hostKeyChanged(host: host, expected: expected, presented: presented, bundled: true)
        case .HostKeyNotConfirmed(let host):
            return SetupProblem(
                problem: "That isn't the fingerprint \(host) showed", nextStep: "Go back and connect again.")
        case .EmptyRepository:
            return SetupProblem(
                problem: "That store is empty",
                nextStep: "Add a password to it on a computer, then try again.",
                action: .retry, actionTitle: "Try again")
        case .NotAPassStore:
            return SetupProblem(
                problem: "That isn't a pass store",
                nextStep: "Nothing there says which key it is encrypted for. Check the address.",
                details: ["No .gpg-id file at the top of the repository"])
        case .KeyCannotOpenStore:
            return SetupProblem(
                problem: "This key can't open this store",
                nextStep: "Its passwords are encrypted for another key. Plug that key in and try again.",
                action: .retry, actionTitle: "Try again")
        case .CannotOpenEntry(let entry, let detail):
            return SetupProblem(
                problem: "Your key couldn't open \(entry)",
                nextStep: "The store is downloaded. Try again; if it keeps failing, that file may be damaged.",
                details: [detail], action: .retry, actionTitle: "Try again")
        case .UnusableKey(let message):
            return SetupProblem(
                problem: "This key can't be used", nextStep: sentence(message))
        case .WrongStep(let message):
            return SetupProblem(
                problem: "Setup got out of step", nextStep: "Go back one step and try again.",
                details: [message])
        case .Failed, .StoreBusy:
            let message = if case .Failed(let message) = error { message } else { storeBusyText }
            if downloading {
                return SetupProblem(
                    problem: "The download didn't finish",
                    nextStep: "Nothing was kept. Try again, or check the address and your account's access.",
                    details: [message], action: .retry, actionTitle: "Try again")
            }
            return SetupProblem(
                problem: "That didn't work", nextStep: "Try again.", details: [message],
                action: .retry, actionTitle: "Try again")
        case .BadAddress(let message):
            return SetupProblem(problem: "Check the address", nextStep: message)
        case .KeyNotAccepted(let host, _):
            return SetupProblem(
                problem: "\(host) doesn't know this key yet", nextStep: "Add its SSH key to your account there.")
        }
    }

    /// "it has no decryption key" as a sentence.
    private static func sentence(_ text: String) -> String {
        guard let first = text.first else { return text }
        let capitalised = first.uppercased() + text.dropFirst()
        return capitalised.hasSuffix(".") ? capitalised : capitalised + "."
    }

    // MARK: The key going in and out (device)

    #if !targetEnvironment(simulator)
    /// Follows the system's smart card readers, as Home does: a reader
    /// appearing on the plug-in screen reads the key.
    private func watchReaders() {
        guard let manager = TKSmartCardSlotManager.default else {
            noSmartCards = true
            return
        }
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
        keyPresent = present
        if !present {
            keyPort.unplugged()
        } else if screen == .plugIn {
            readKey()
        } else {
            attachKey()
        }
    }
    #endif
}
