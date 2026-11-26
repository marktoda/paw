import Foundation
import Observation
import SwiftUI

/// The screen setup shows. The core's state picks it; two of the core's
/// steps have two screens each (the key, then the address; the address,
/// then the host's answer).
enum SetupScreen: Equatable {
    case welcome
    case plugIn
    case yourKey(KeySummary)
    case address(notice: AddressNotice?)
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

/// The key sheet while setup shows it; its stage is `SetupModel.keyTask`'s.
struct SetupSheet: Identifiable {
    enum Purpose {
        case download, check
    }

    let id = UUID()
    let purpose: Purpose
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

/// Reading the key failed before it was read: the session could not be
/// opened (design.md 2.11).
private struct SetupAttachFailure: Error {
    let error: Error
}

/// Drives the core's `Setup` for the setup screens. The core decides every
/// step; this keeps what the screens need besides: which of a step's two
/// screens is up, the problem to show, and the key sheet. The PIN goes
/// from the sheet's field straight to the core task, never kept here.
@MainActor @Observable
final class SetupModel {
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
    /// The key going in and out, and reading it.
    let reader: KeyReader
    /// The key sheet's task: the download, or the check of the key.
    let keyTask: KeyTaskDriver
    let simulator: SetupSimulator?
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
    private(set) var readiness: SetupWriteReadiness?
    private(set) var storeFacts: StoreFacts?
    /// Hex fingerprint of the key's SSH key, once read this launch.
    private(set) var sshFingerprint: String?
    /// A download was cancelled while the key waited for its touch: the
    /// key still waits. The simulator's download screen offers to time it
    /// out; on a device the key's own timeout ends it.
    private(set) var touchLeftWaiting = false

    private let workQueue = DispatchQueue(label: "paw-setup", qos: .userInitiated)

    init(setup: Setup, repoPath: String, keyPort: KeyPort, connect: @escaping KeyReader.Connect,
         simulator: SetupSimulator?, finished: @escaping () -> Void, startOver: @escaping () -> Void) {
        self.setup = setup
        self.repoPath = repoPath
        self.keyPort = keyPort
        let reader = KeyReader(port: keyPort, connect: connect)
        self.reader = reader
        keyTask = KeyTaskDriver(reader: reader, waitsForLetGoTasks: true)
        self.simulator = simulator
        self.finished = finished
        self.startOver = startOver
        let state = setup.state()
        self.state = state
        started = state != .readKey
        if case .checkHost(let address, _) = state {
            addressText = address
        }
        if case .done = state {
            learnAboutStore()
        }
        keyTask.owner = self
        reader.onPresent = { [weak self] in
            self?.keyCameIn()
        }
        #if DEBUG && targetEnvironment(simulator)
        if let simulator {
            reader.plugsInSimulatedKey = simulator.plugIn
        }
        #endif
        reader.watch(startsUnplugged: simulator?.startsUnplugged ?? true)
        #if DEBUG && targetEnvironment(simulator)
        // Before the key is read, reading it opens the session, and shows
        // on Plug in if that fails.
        if state != .readKey { reader.readIfNeeded() }
        #endif
    }

    /// A key is in (a reader is present, or the simulated key is plugged).
    var keyPresent: Bool {
        reader.present
    }

    /// This iPhone has no smart card support at all (device only).
    var noSmartCards: Bool {
        reader.unsupported
    }

    /// Why the key in could not be read (its session not opened), or that
    /// this iPhone can't read one, for the Plug in screen.
    var attachProblem: KeyErrorWords? {
        reader.problem.map { KeyErrorWords($0) }
    }

    /// A key came in: on Plug in, reading it is the step; elsewhere (a
    /// relaunch past the first step) its session is opened, so the key
    /// sheet finds it.
    private func keyCameIn() {
        if screen == .plugIn, sheet == nil {
            readKey()
        } else {
            reader.readIfNeeded()
        }
    }

    /// "Try again" under the attach problem on Plug in.
    func retryAttach() {
        reader.clearFailure()
        if screen == .plugIn {
            readKey()
        } else {
            reader.readIfNeeded()
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
            reader.read()
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
        return readStoreAddress(text: typed)
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
        reader.clearFailure()
        working = "Reading your key\u{2026}"
        let reader = reader
        let setup = setup
        workQueue.async {
            let result: Result<(SetupState, String?), Error> = Result {
                let session: KeySession
                do {
                    session = try reader.openSession()
                } catch {
                    throw SetupAttachFailure(error: error)
                }
                let info = session.info()
                return (try setup.readKeyFromSession(session: session), info.authenticationFingerprint)
            }
            let handoff = Handoff(value: result)
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
            reader.readFailed(failure.error)
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

    enum Moment {
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
            let handoff = Handoff(value: Result { try work(setup) })
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
        sheet = SetupSheet(purpose: purpose)
        let setup = setup
        let port = keyPort
        // A download stopped while the key waited for a touch is still
        // being removed: the driver starts this one once it is.
        keyTask.start { (listener: KeyTaskListener) -> KeyTask in
            let task: KeyTask = switch purpose {
            case .download: setup.downloadWithKey(port: port, listener: listener)
            case .check: setup.testDecryptWithKey(port: port, listener: listener)
            }
            BackgroundTime.hold("Setup", while: task)
            return task
        }
    }

    /// Cancel always works. A download stopped part way leaves nothing: the
    /// core removes it and setup is back at the download step.
    func cancelSheet() {
        if sheet != nil, keyTask.stage == .touch {
            // The key's command can't be interrupted: it still waits.
            touchLeftWaiting = true
        }
        keyTask.cancel()
        sheet = nil
    }

    #if DEBUG && targetEnvironment(simulator)
    /// The debug control on the download screen after a cancel during the
    /// touch: the key's wait runs out now.
    func simulatedTouchTimeout() {
        touchLeftWaiting = false
        SimulatedKeyHolder.shared.key.timeOutTouch()
    }
    #endif

    private var entryName: String {
        if case .checkKey(let entry?)? = keyTask.taskPurpose, !entry.isEmpty {
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

    private func succeeded() {
        state = setup.state()
        if case .done = state {
            learnAboutStore()
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
                    problem: SyncFailure.refusedRemoteProblem,
                    nextStep: "Rename or remove them on a computer, then try again.",
                    details: refused.map { "\($0.path): \(refusedWords($0))" },
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
           let remote = setup.lastRemoteMessage(), let words = SetupWords.remoteWords(remote) {
            problem = words
            return
        }
        switch error {
        case .BadAddress(let problem):
            addressProblem = addressProblemWords(problem)
        case .KeyNotAccepted(let host, let publicKey):
            notAccepted = KeyNotAccepted(host: host, publicKey: publicKey)
        case .HostKeyChanged(let host, let expected, let presented):
            let bundled = readStoreAddress(text: addressSet)?.hostKeyBundled ?? false
            problem = SetupWords.hostKeyChanged(host: host, expected: expected, presented: presented, bundled: bundled)
        case .HostUnreachable:
            var words = SetupWords.words(error, during: moment)
            words.offer = setup.portFallback().map(SetupWords.portFallbackOffer)
            problem = words
        case .HostKeyBundled(let host) where moment == .forgettingHostKey:
            problem = SetupWords.hostKeyKept(hostKeyBundled(host))
        case .Failed(let message) where moment == .forgettingHostKey:
            problem = SetupWords.hostKeyKept(message)
        case .StoreBusy where moment == .forgettingHostKey:
            problem = SetupWords.hostKeyKept(storeBusyText)
        default:
            problem = SetupWords.words(error, during: moment)
        }
    }
}

extension SetupModel: KeyTaskOwner {
    var keyWords: KeyErrorWords.Context {
        KeyErrorWords.Context(entry: entryName, purpose: sheet?.purpose == .download ? .download : .check)
    }

    func workingStage(_ progress: TaskProgress?) -> KeySheetStage {
        let purpose = sheet?.purpose ?? .check
        // Re-read on every change: objects and bytes, when known.
        let transfer = purpose == .download ? setup.transferProgress() : nil
        let line = transfer.flatMap(Self.transferLine) ?? progress.map(Self.progressLine)
        return .working(message: workingMessage(purpose, started: progress != nil || transfer != nil), progress: line)
    }

    func keyStageShown(_ stage: KeySheetStage) {
        if stage == .touch {
            // A new command waits for a touch, so any cancelled one has ended.
            touchLeftWaiting = false
        }
    }

    func keyTaskEnded(_ end: KeyTaskEnd) -> KeySheetStage? {
        guard let sheet else {
            state = setup.state()
            return nil
        }
        switch end.result {
        case .done:
            succeeded()
            return .done
        case .setupFailed(let error):
            // Not the key's problem: the host, the repository or the store.
            self.sheet = nil
            stopped(sheet.purpose, because: error)
            return nil
        case .cancelled, .syncFailed:
            self.sheet = nil
            state = setup.state()
            return nil
        case .failed(.Reader):
            // The sheet says what the key answered; setup is where it was.
            state = setup.state()
            if sheet.purpose == .download {
                simulator?.downloadFailed()
            }
            return standardStage(end.result)
        case .failed:
            return standardStage(end.result)
        }
    }

    /// A task the sheet let go of has ended: show where setup is.
    func letGoTaskEnded(_ end: KeyTaskEnd) {
        state = setup.state()
    }

    func doneShown() {
        sheet = nil
    }
}
