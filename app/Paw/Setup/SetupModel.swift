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

    /// The sheet's run: the same across Try again.
    let id: UUID
    let purpose: Purpose
}

/// Drives the core's `Setup` for the setup screens. The core decides every
/// step. The PIN goes from the sheet's field straight to the core task,
/// never kept here.
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
    let repoPath: String
    /// Shared with Home afterwards, so the PIN typed here is still held.
    let keyPort: KeyPort
    let reader: KeyReader
    let keyTask: KeyTaskDriver
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
    private var sheetPurpose: SetupSheet.Purpose = .download
    private(set) var readiness: SetupWriteReadiness?
    private(set) var storeFacts: StoreFacts?
    /// The store's lists of keys couldn't be read for the done screen.
    private(set) var whoCanOpenUnread = false
    /// Hex fingerprint of the key's SSH key, once read this launch.
    private(set) var sshFingerprint: String?
    /// "Set up", Plug in or Try again asked for the key to be read: the
    /// next read that works is setup's first step.
    @ObservationIgnored private var readingKey = false

    private let workQueue = DispatchQueue(label: "paw-setup", qos: .userInitiated)

    init(setup: Setup, repoPath: String, keyPort: KeyPort, keys: any KeyConnector,
         finished: @escaping () -> Void, startOver: @escaping () -> Void) {
        self.setup = setup
        self.repoPath = repoPath
        self.keyPort = keyPort
        let reader = KeyReader(port: keyPort, source: keys)
        self.reader = reader
        keyTask = KeyTaskDriver(reader: reader)
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
        reader.onRead = { [weak self] read in
            self?.keyWasRead(read)
        }
        // A key already in is read now; a failure shows on Plug in.
        reader.watch()
    }

    var sheet: SetupSheet? {
        keyTask.shown.map { SetupSheet(id: $0, purpose: sheetPurpose) }
    }

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

    /// The key was read, or reading it failed. On Plug in, a key read is
    /// setup's first step: a key coming in, or "Set up", Plug in and Try
    /// again asking for it. Elsewhere (a relaunch past the first step) the
    /// session is only opened, so the key sheet finds it.
    private func keyWasRead(_ read: Result<KeySession, Error>) {
        let asked = readingKey
        readingKey = false
        switch read {
        case .failure:
            // The reader keeps the failure for Plug in to say.
            if asked { working = nil }
        case .success(let session):
            guard screen == .plugIn, sheet == nil, asked || working == nil else { return }
            stepThroughKey(session)
        }
    }

    /// "Try again" under the attach problem on Plug in.
    func retryAttach() {
        if screen == .plugIn {
            readKey()
        } else {
            reader.retry()
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

    /// "Set up" on the welcome screen: on to the key.
    func begin() {
        problem = nil
        started = true
        readKey()
    }

    /// Reads the plugged-in key: no PIN, no touch. A session already open
    /// is kept, with the PIN it holds.
    func readKey() {
        guard keyPresent, working == nil else { return }
        problem = nil
        working = "Reading your key\u{2026}"
        readingKey = true
        reader.retry()
    }

    private func stepThroughKey(_ session: KeySession) {
        problem = nil
        working = "Reading your key\u{2026}"
        let setup = setup
        let handoff = Handoff(value: session)
        workQueue.async {
            let session = handoff.value
            let result: Result<(SetupState, String?), Error> = Result {
                (try setup.readKeyFromSession(session: session), session.info().authenticationFingerprint)
            }
            let read = Handoff(value: result)
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    self?.keyRead(read.value)
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
        case .failure(let error):
            state = setup.state()
            show(error)
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
        run("Checking \(host.isEmpty ? "the address" : host)\u{2026}") { setup in
            _ = try setup.setAddress(address: typed)
            return try setup.checkHost()
        }
    }

    /// "Trust this server" on the host's fingerprint.
    func trustHost(fingerprint: String) {
        problem = nil
        run("Checking \(host)\u{2026}") { setup in
            try setup.confirmHost(fingerprint: fingerprint)
        }
    }

    /// "Check again" once the key was added on the host.
    func checkAgain() {
        problem = nil
        run("Checking \(host)\u{2026}") { setup in
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
        run("Checking \(host)\u{2026}") { setup in
            _ = try setup.forgetHostKey()
            return try setup.checkHost()
        } failed: { [weak self] error in
            self?.hostKeyNotForgotten(error)
        }
    }

    /// "Try port 443": the address becomes the host's other endpoint, and
    /// the address screen says so under the field.
    func usePortFallback() {
        problem = nil
        run(nil) { setup in
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
            run(nil) { setup in try setup.back() } then: { [weak self] _ in
                self?.started = false
            }
        case .address:
            if case .address = state {
                keyConfirmed = false
            } else {
                run(nil) { setup in try setup.back() }
            }
        case .confirmHost, .download, .check:
            run(nil) { setup in try setup.back() } then: { [weak self] state in
                if case .checkHost(let address, _) = state {
                    self?.addressText = address
                }
            }
        }
    }

    /// "Open my passwords" on the last screen. Home follows the key with
    /// its own reader.
    func openStore() {
        reader.stopWatching()
        finished()
    }

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

    /// Runs a plain core step off the main thread; `then` sees its new
    /// state, `failed` why it stopped (said as `show` says it unless given).
    private func run(_ message: String?,
                     _ work: @escaping @Sendable (Setup) throws -> SetupState,
                     then: @escaping (SetupState) -> Void = { _ in },
                     failed: ((Error) -> Void)? = nil) {
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
                        if let failed {
                            failed(error)
                        } else {
                            self.show(error)
                        }
                    }
                }
            }
        }
    }

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
        guard !keyTask.isShown else { return }
        problem = nil
        sheetPurpose = purpose
        keyTask.start(job(purpose))
    }

    /// Cancel always works. A download stopped part way leaves nothing: the
    /// core removes it and setup is back at the download step.
    func cancelSheet() {
        keyTask.cancel()
    }

    private var entryName: String {
        if case .checkKey(let entry?)? = keyTask.taskPurpose, !entry.isEmpty {
            return EntryRef(path: entry).name
        }
        return "your store"
    }

    /// Setup's step after a key task: the core may have moved on, or put
    /// back a download stopped part way.
    private func readState() {
        state = setup.state()
        if case .done = state {
            learnAboutStore()
        }
    }

    /// For the done screen: whether saving works, and what is in the store.
    private func learnAboutStore() {
        var unread = false
        do {
            readiness = try setup.writeReadiness()
        } catch {
            unread = true
        }
        do {
            let store = try PassStore(dir: repoPath)
            let recipients: [String]
            do {
                recipients = try store.recipients(folder: "")
            } catch {
                recipients = []
                unread = true
            }
            storeFacts = StoreFacts(folders: store.home().folders.count, recipients: recipients.count)
        } catch {
            unread = true
        }
        whoCanOpenUnread = unread
    }

    private func show(_ error: Error, downloading: Bool = false) {
        guard let error = error as? SetupError else {
            problem = SetupProblem(
                problem: "Setup stopped", nextStep: "Try again.", details: [describe(error)], action: .retry,
                actionTitle: "Try again")
            return
        }
        switch error {
        case .BadAddress(let problem):
            addressProblem = addressProblemWords(problem)
        case .Fault(.remote(.keyNotAccepted(let host, let publicKey))):
            notAccepted = KeyNotAccepted(host: host, publicKey: publicKey)
        default:
            problem = SetupWords.words(error, downloading: downloading)
        }
    }

    /// "I expected this change" stopped: the server's key is kept, and
    /// the screen says why when the core gave a reason.
    private func hostKeyNotForgotten(_ error: Error) {
        switch error as? SetupError {
        case .Fault(.remote(.hostKeyBundled(let host)))?:
            problem = SetupWords.hostKeyKept(hostKeyBundled(host))
        case .Fault(.local(let message))?:
            problem = SetupWords.hostKeyKept(message)
        case .Fault(.storeBusy)?:
            problem = SetupWords.hostKeyKept(StoreBusyWords.line)
        default:
            show(error)
        }
    }
}

extension SetupModel {
    private func job(_ purpose: SetupSheet.Purpose) -> KeyJob<SetupError> {
        let setup = setup
        let port = keyPort
        return KeyJob(
            begin: { listener in
                switch purpose {
                case .download: setup.downloadWithKey(port: port, listener: listener)
                case .check: setup.testDecryptWithKey(port: port, listener: listener)
                }
            },
            words: { [weak self] in
                KeyErrorWords.Context(entry: self?.entryName ?? "your store", purpose: purpose == .download ? .download : .check)
            },
            working: { [weak self] progress in
                self?.working(purpose, progress) ?? ("", nil)
            },
            ended: { [weak self] end in
                self?.ended(end, purpose: purpose) ?? .close
            },
            afterEnd: { [weak self] in
                self?.readState()
            },
            hold: { BackgroundTime.begin("Setup") },
            // A download stopped while the key waited for a touch is still
            // being removed: this one starts once it is.
            waitsForLetGoTasks: true)
    }

    private func working(_ purpose: SetupSheet.Purpose, _ progress: TaskProgress?) -> (message: String, progress: String?) {
        let line = progress.flatMap(SetupWords.transferLine) ?? progress.map(\.counted)
        return (SetupWords.working(purpose, host: host, started: progress != nil), line)
    }

    private func ended(_ end: KeyTaskEnd<SetupError>, purpose: SetupSheet.Purpose) -> KeySheetAfterEnd {
        switch end {
        case .done, .failed:
            return .standard
        case .stepFailed(let error):
            // Not the key's problem: the host, the repository or the store.
            show(error, downloading: purpose == .download)
            return .close
        case .cancelled:
            return .close
        }
    }
}
