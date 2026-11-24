import Foundation
import Observation
#if !targetEnvironment(simulator)
import CryptoTokenKit
#endif

/// What the provider was asked for.
enum AutoFillRequest: Equatable, Sendable {
    /// The key icon or "Passwords…": a list for these sites (web addresses
    /// or domains, as iOS gives them).
    case list(services: [String])
    /// A suggestion above the keyboard was tapped: one entry, by path.
    case entry(path: String)
}

/// Where a request ends. The extension passes these to iOS; the debug
/// harness records them. Called once per request.
@MainActor
protocol AutoFillCompletion: AnyObject {
    func fill(user: String, password: String)
    func end(_ reason: AutoFillEnd)
}

/// Why a picked entry asks before it fills.
enum AutoFillConfirmReason: Equatable, Sendable {
    /// Not one of this site's entries (found by search).
    case otherSite
    /// Named like the site (`bankofamerica` on `bankofamerica.com`) and
    /// never filled on this domain.
    case firstTime(site: String)
    /// Named like the site, but filled only on other domains of that name:
    /// `known` is `bankofamerica.com`, this is `bankofamerica.co`.
    case lookalike(site: String, known: [String])
}

/// A Face ID problem from the last tap, said at the top of the list.
struct AutoFillNotice: Equatable {
    let problem: String
    let nextStep: String
    /// Offers to fill this entry with the key instead.
    var keyEntry: EntryRef? = nil
}

/// Hands a core task's stage changes to the main queue without blocking.
private final class TaskListener: KeyTaskListener, @unchecked Sendable {
    private let changed: @Sendable () -> Void

    init(changed: @escaping @Sendable () -> Void) {
        self.changed = changed
    }

    func stateChanged(state: KeyTaskState) {
        DispatchQueue.main.async(execute: changed)
    }
}

/// A value that crosses from a worker queue to the main actor once.
private struct Carry<Value>: @unchecked Sendable {
    let value: Value
}

/// The AutoFill screens' one model: the list for a site, search, and a
/// fill by Face ID copy or by the key. The same code runs in the extension
/// and, in debug builds, in the app's harness (`-pawHostProvider`), with
/// the completion injected.
///
/// The password never reaches a view: it goes from the opened entry
/// straight to the completion, and the entry is wiped at once. File and
/// card work runs off the main thread.
@MainActor @Observable
final class AutoFillModel {
    enum Screen: Equatable {
        case loading
        case list
        /// The entry is not one of this site's, or is by name only and not
        /// yet filled here: fill it anyway, or not.
        case confirm(EntryRef, AutoFillConfirmReason)
        /// Face ID is up for this entry.
        case faceId(EntryRef)
        /// The key sheet, for `keyEntry`.
        case key
        /// Handed to the completion.
        case filled
        case noStore
        /// paw is using the store; Try again opens it again.
        case busy
        /// One short line and one next step, with Close; and "Back to the
        /// list" when the list is where the next step is.
        case problem(String, String, backToList: Bool)
    }

    /// How long an entry filled with the key stays available to Face ID,
    /// as in the app.
    static let faceIdWindowSecs: UInt32 = 8 * 60 * 60

    let request: AutoFillRequest
    private(set) var screen: Screen = .loading
    /// The site the list is for, e.g. `github.com`. `nil` for an app iOS
    /// gives no site for.
    private(set) var site: String?
    /// The site's own entries, under "For <site>": by host, or by a name
    /// already filled on this domain. These fill without asking.
    private(set) var matches: [EntryRef] = []
    /// Named like the site but never filled on this domain, listed apart
    /// under "Might be for this site"; each asks first. A name match
    /// paired only with other domains (a lookalike) is in neither list and
    /// is found by search only.
    private(set) var maybe: [EntryRef] = []
    /// Every match for the site by path, lookalikes included, with why.
    @ObservationIgnored private var found: [String: MatchedEntry] = [:]
    /// The name match the person confirmed for this domain, recorded once
    /// it has filled: (entry path, registrable domain).
    @ObservationIgnored private var pairing: (path: String, site: String)?
    var query = "" {
        didSet { search() }
    }
    private(set) var hits: [SearchHit] = []
    /// Entries with a live Face ID copy.
    private(set) var live: Set<String> = []
    private(set) var notice: AutoFillNotice?
    private(set) var keyEntry: EntryRef?
    private(set) var keyStage: KeySheetStage = .busy
    /// Typed in the key sheet's PIN field; handed to the core on Continue
    /// and cleared at once.
    var pinEntry = ""
    /// Key tasks not yet ended, for the harness's "task ended" signal.
    private(set) var pendingKeyTasks = 0
    /// Face ID copies can be kept: the key sheet says fills afterwards use Face ID.
    private(set) var keepsCopies = false

    @ObservationIgnored private let open: @Sendable () -> AutoFillOpening
    @ObservationIgnored private var source: AutoFillSource?
    @ObservationIgnored private weak var completion: AutoFillCompletion?
    /// The task the sheet follows, and those it let go of, until each ends.
    @ObservationIgnored private var followed: UUID?
    @ObservationIgnored private var tasks: [UUID: KeyTask] = [:] {
        didSet { pendingKeyTasks = tasks.count }
    }
    /// The entry each task opens.
    @ObservationIgnored private var taskEntries: [UUID: String] = [:]
    @ObservationIgnored private var attachProblem: (problem: String, nextStep: String)?
    @ObservationIgnored private var readerPresent = false
    @ObservationIgnored private var reading = false
    @ObservationIgnored private var started = false
    @ObservationIgnored private var finished = false
    #if !targetEnvironment(simulator)
    @ObservationIgnored private var slotWatch: NSKeyValueObservation?
    #endif
    @ObservationIgnored private let work = DispatchQueue(label: "paw-autofill", qos: .userInitiated)
    @ObservationIgnored private let cardQueue = DispatchQueue(label: "paw-card", qos: .userInitiated)

    init(request: AutoFillRequest, completion: AutoFillCompletion,
         open: @escaping @Sendable () -> AutoFillOpening) {
        self.request = request
        self.completion = completion
        self.open = open
    }

    /// Opens the store off the main thread, then shows the list (or, for
    /// a suggestion, goes straight to Face ID or the key).
    func start() {
        guard !started else { return }
        started = true
        let open = open
        let request = request
        work.async {
            let opening = open()
            var live: [String] = []
            var matches: [EntryRef] = []
            var maybe: [EntryRef] = []
            var found: [MatchedEntry] = []
            if case .ready(let source) = opening {
                live = ((try? source.copies?.list()) ?? []).map(\.entry)
                switch request {
                case .list(let services):
                    found = SiteMatch.entries(in: source.store, for: services, pairings: source.pairings)
                    matches = found.filter { SiteMatch.isTrusted($0.kind) }.map(\.entry)
                    maybe = found.filter { $0.kind == .name(pairing: .unpaired) }.map(\.entry)
                case .entry(let path):
                    matches = SiteMatch.entry(path, in: source.store).map { [$0] } ?? []
                }
            }
            let carried = Carry(value: (opening, live, matches, found, maybe))
            DispatchQueue.main.async {
                MainActor.assumeIsolated { [weak self] in
                    self?.found = Dictionary(carried.value.3.map { ($0.entry.path, $0) }) { first, _ in first }
                    self?.maybe = carried.value.4
                    self?.opened(carried.value.0, live: carried.value.1, matches: carried.value.2)
                }
            }
        }
    }

    private func opened(_ opening: AutoFillOpening, live: [String], matches: [EntryRef]) {
        switch opening {
        case .noStore:
            screen = .noStore
        case .busy:
            screen = .busy
        case .unreadable:
            screen = .problem("paw couldn't read your store", "Open paw to fix it.", backToList: false)
        case .ready(let source):
            self.source = source
            keepsCopies = source.copies != nil
            self.live = Set(live)
            self.matches = matches
            switch request {
            case .list(let services):
                site = services.lazy.compactMap(SiteMatch.host(of:)).first
                screen = .list
            case .entry:
                guard let entry = matches.first else {
                    end(.credentialIdentityNotFound)
                    return
                }
                screen = .list
                pick(entry)
            }
        }
    }

    /// Try again after "The store is busy".
    func reopen() {
        guard screen == .busy else { return }
        screen = .loading
        started = false
        start()
    }

    // MARK: The list

    private var isSuggestion: Bool {
        if case .entry = request { return true }
        return false
    }

    private func search() {
        guard let source else { return }
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        hits = trimmed.isEmpty ? [] : source.store.search(query: trimmed, limit: 100)
    }

    var isSearching: Bool {
        !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// Whether the entry is one of the site's, as matched by name. A
    /// suggestion above the keyboard is one by construction: iOS offers it
    /// only for the site it was published for.
    func isMatch(_ entry: EntryRef) -> Bool {
        isSuggestion || matches.contains { $0.path == entry.path }
    }

    /// Whether the entry is named like the site but not yet filled here.
    func isMaybe(_ entry: EntryRef) -> Bool {
        !isSuggestion && maybe.contains { $0.path == entry.path }
    }

    /// What a fill says it does: "Fill github.com", or with the site when
    /// the entry's name does not already say it, "Fill fastmail on
    /// example.com". The key sheet and the Face ID prompt both say this.
    func purpose(_ entry: EntryRef) -> String {
        guard let site, site != entry.name.lowercased() else { return "Fill \(entry.name)" }
        return "Fill \(entry.name) on \(site)"
    }

    /// A tap on an entry. One that is not this site's asks first (design
    /// 2.10): a search on a lookalike site must not hand over the real
    /// site's password with a single Face ID glance. So does one that
    /// matches by name only, until it has filled on this domain.
    func pick(_ entry: EntryRef) {
        guard screen == .list else { return }
        notice = nil
        pairing = nil
        if isSuggestion {
            open(entry)
            return
        }
        guard let match = found[entry.path] else {
            screen = .confirm(entry, .otherSite)
            return
        }
        switch match.kind {
        case .exactHost, .sameDomain, .name(.paired):
            open(entry)
        case .name(.unpaired):
            screen = .confirm(entry, .firstTime(site: match.site))
        case .name(.pairedElsewhere(let known)):
            screen = .confirm(entry, .lookalike(site: match.site, known: known))
        }
    }

    /// "Fill anyway" or "Use it here" on the confirm step. A name match
    /// confirmed the first time is paired with the domain once it fills.
    /// Nothing else ever is: not a password from search for another site,
    /// and not a lookalike of a domain the entry is paired with, which
    /// fills this once and warns every time (the core refuses it too).
    func fillAnyway() {
        guard case .confirm(let entry, let reason) = screen else { return }
        switch reason {
        case .firstTime(let site):
            pairing = (entry.path, site)
        case .lookalike, .otherSite:
            pairing = nil
        }
        screen = .list
        open(entry)
    }

    /// Cancel on the confirm step, or "Back to the list" after a problem:
    /// the list again, nothing filled.
    func backToList() {
        pairing = nil
        switch screen {
        case .confirm, .problem(_, _, backToList: true): screen = .list
        default: break
        }
    }

    /// Face ID if the entry has a live copy, else the key.
    private func open(_ entry: EntryRef) {
        if live.contains(entry.path), source?.copies != nil {
            openWithFaceId(entry)
        } else {
            openWithKey(entry)
        }
    }

    /// Cancel in the toolbar, or Close: the request ends, nothing filled.
    func cancel() {
        end(.userCanceled)
    }

    // MARK: Face ID

    private func openWithFaceId(_ entry: EntryRef) {
        guard let source, let copies = source.copies else { return }
        screen = .faceId(entry)
        let store = source.store
        let reason = purpose(entry)
        work.async {
            let result = Result {
                try store.openEntryWithFaceId(copies: copies, entry: entry.path, reason: reason)
            }
            let carried = Carry(value: result)
            DispatchQueue.main.async {
                MainActor.assumeIsolated { [weak self] in
                    self?.faceIdFinished(entry, carried.value)
                }
            }
        }
    }

    private func faceIdFinished(_ entry: EntryRef, _ result: Result<FaceIdOpen, Error>) {
        guard !finished else {
            if case .success(.opened(let opened)) = result { opened.close() }
            return
        }
        switch result {
        case .success(.opened(let opened)):
            fill(opened, entry: entry)
        case .success(.needsKey):
            live.remove(entry.path)
            screen = .list
            openWithKey(entry)
        case .failure(let error):
            if case .FaceIdCancelled? = error as? PawError, isSuggestion {
                end(.userCanceled)
                return
            }
            screen = .list
            notice = Self.faceIdNotice(error, entry: entry)
        }
    }

    private static func faceIdNotice(_ error: Error, entry: EntryRef) -> AutoFillNotice {
        if isStoreBusy(message: String(describing: error)) {
            return AutoFillNotice(problem: "The store is busy", nextStep: "Tap \(entry.name) again.")
        }
        switch error as? PawError {
        case .FaceIdCancelled?:
            return AutoFillNotice(problem: "Face ID was cancelled", nextStep: "Tap \(entry.name) to try again.",
                                  keyEntry: entry)
        case .FaceIdNotRecognised?:
            return AutoFillNotice(problem: "Face ID didn't recognise you", nextStep: "Tap \(entry.name) to try again.",
                                  keyEntry: entry)
        case .FaceIdLockedOut?:
            return AutoFillNotice(problem: "Face ID is locked", nextStep: "Lock and unlock your iPhone, then try again.",
                                  keyEntry: entry)
        case .FaceIdChanged?:
            return AutoFillNotice(problem: "Face ID changed on this iPhone", nextStep: "", keyEntry: entry)
        default:
            return AutoFillNotice(problem: "Face ID didn't open \(entry.name)", nextStep: "", keyEntry: entry)
        }
    }

    // MARK: The key

    /// Fills with the key: the sheet at the first stage still needed.
    func openWithKey(_ entry: EntryRef) {
        guard let source, followed == nil else { return }
        notice = nil
        keyEntry = entry
        keyStage = .busy
        screen = .key
        startTask(entry, source: source)
        readKey()
    }

    private func startTask(_ entry: EntryRef, source: AutoFillSource) {
        pinEntry = ""
        let id = UUID()
        let listener = TaskListener { [weak self] in
            MainActor.assumeIsolated {
                self?.taskChanged(id)
            }
        }
        followed = id
        taskEntries[id] = entry.path
        // A copy is kept after the key opens the entry, as in the app: the
        // copy cache is safe for two processes (core/src/copies/dir.rs).
        tasks[id] = source.keyPort.openEntry(
            store: source.store, entry: entry.path, copies: source.copies, ttlSecs: Self.faceIdWindowSecs,
            listener: listener)
        taskChanged(id)
    }

    func continueWithPin() {
        guard let id = followed, let task = tasks[id], !pinEntry.isEmpty else { return }
        switch keyStage {
        case .pin, .wrongPin, .pinAgain: break
        default: return
        }
        task.supplyPin(pin: Data(pinEntry.utf8))
        pinEntry = ""
    }

    /// Try again: read the key again if it could not be read, else a new task.
    func retry() {
        guard let source, let entry = keyEntry, let id = followed else { return }
        if case .plugIn? = tasks[id]?.state() {
            attachProblem = nil
            keyStage = .plugIn
            readKey()
            return
        }
        letGo(id)
        startTask(entry, source: source)
    }

    /// Cancel on the key sheet: back to the list, or, for a suggestion
    /// (which shows nothing else), the request ends.
    func cancelKey() {
        if let id = followed {
            letGo(id)
        }
        pinEntry = ""
        keyEntry = nil
        source?.keyPort.session()?.forgetPin()
        if isSuggestion {
            end(.userCanceled)
        } else {
            screen = .list
        }
    }

    private func taskChanged(_ id: UUID) {
        guard let task = tasks[id] else { return }
        let state = task.state()
        guard id == followed, let entry = keyEntry, !finished else {
            settle(id, state)
            return
        }
        if Self.isFinal(state) {
            tasks[id] = nil
            taskEntries[id] = nil
            followed = nil
        }
        switch state {
        case .plugIn:
            if let attachProblem {
                keyStage = .failed(problem: attachProblem.problem, nextStep: attachProblem.nextStep)
            } else {
                keyStage = .plugIn
            }
        case .pin(let triesLeft, let forgotten, let lastError):
            let tries = Int(triesLeft ?? 3)
            switch lastError {
            case nil:
                keyStage = .pin(triesLeft: tries, forgotten: forgotten)
            case .WrongPin(let left)?:
                keyStage = .wrongPin(triesLeft: Int(left))
            case let error?:
                let words = KeyErrorWords(pinAskedAgain: error, entry: entry.name)
                keyStage = .pinAgain(triesLeft: tries, problem: words.problem, nextStep: words.nextStep)
            }
        case .busy:
            keyStage = .busy
        case .touch:
            keyStage = .touch
        case .working:
            keyStage = .working(message: "Opening \(entry.name)\u{2026}", progress: nil)
        case .done(.opened(let opened, _)):
            keyStage = .done
            fill(opened, entry: entry)
        case .done:
            keyStage = .done
        case .failed(.NotForThisKey) where task.keyMismatch() != nil:
            keyStage = .failed(
                problem: "This isn't your store's key",
                nextStep: "Plug in the key you set paw up with.")
        case .failed(.PinBlocked):
            keyStage = .locked
        case .failed(.Cancelled), .cancelled:
            break
        case .failed(let error) where isStoreBusy(message: String(describing: error)):
            keyStage = .failed(problem: "The store is busy", nextStep: "Try again in a moment.")
        case .failed(let error):
            if case .KeyRemoved = error {
                readKey()
            }
            let words = KeyErrorWords(error, entry: entry.name)
            keyStage = .failed(problem: words.problem, nextStep: words.nextStep)
        }
        switch keyStage {
        case .pin, .wrongPin, .pinAgain: break
        default: pinEntry = ""
        }
    }

    /// The sheet stops following a task: it is cancelled and settled when
    /// it ends.
    private func letGo(_ id: UUID) {
        if followed == id {
            followed = nil
        }
        guard let task = tasks[id] else { return }
        task.cancel()
        settle(id, task.state())
    }

    /// A task the sheet let go of has ended. One that finished anyway (in
    /// the last moment after the card answered) is wiped, and the copy it
    /// kept is removed: nothing was asked for.
    private func settle(_ id: UUID, _ state: KeyTaskState) {
        guard Self.isFinal(state) else { return }
        tasks[id] = nil
        let path = taskEntries.removeValue(forKey: id)
        guard case .done(.opened(let opened, let until)) = state else { return }
        opened.close()
        if until != nil, let copies = source?.copies, let path {
            let carried = Carry(value: copies)
            work.async { try? carried.value.forgetEntry(entry: path) }
        }
    }

    // MARK: Reading the key

    /// Reads the key if one is plugged in and not yet read; starts watching
    /// for one the first time. Only when the key is needed: a list alone
    /// never touches the card.
    private func readKey() {
        watchReaders()
        guard readerPresent, !reading, let source, source.keyPort.session() == nil else { return }
        reading = true
        let port = source.keyPort
        let connect = source.connect
        cardQueue.async {
            let failure: Error?
            do {
                _ = try port.pluggedIn(connection: connect())
                failure = nil
            } catch {
                failure = error
            }
            let carried = Carry(value: failure)
            DispatchQueue.main.async {
                MainActor.assumeIsolated { [weak self] in
                    self?.readFinished(carried.value)
                }
            }
        }
    }

    private func readFinished(_ failure: Error?) {
        reading = false
        attachProblem = failure.map { _ in
            (problem: "paw couldn't read your key", nextStep: "Unplug it, plug it back in, then try again.")
        }
        if let id = followed {
            taskChanged(id)
        }
    }

    private func watchReaders() {
        #if targetEnvironment(simulator)
        if !readerPresent, !SimulatedKeyHolder.shared.startsUnplugged {
            readerPresent = true
        }
        #else
        guard slotWatch == nil, let manager = TKSmartCardSlotManager.default else { return }
        slotWatch = manager.observe(\.slotNames, options: [.initial, .new]) { [weak self] manager, _ in
            let present = !manager.slotNames.isEmpty
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.readersChanged(present: present)
                }
            }
        }
        #endif
    }

    private func readersChanged(present: Bool) {
        readerPresent = present
        if present {
            readKey()
        } else {
            source?.keyPort.unplugged()
            if let id = followed {
                taskChanged(id)
            }
        }
    }

    #if targetEnvironment(simulator)
    /// The sheet's "plug in the key" control.
    func simulatedPlugIn() {
        SimulatedKeyHolder.shared.key.plugIn()
        readersChanged(present: true)
    }
    #endif

    // MARK: Ending

    /// Hands the credential over and wipes the entry. The password is in
    /// this function's locals only, never in a view or the pasteboard.
    private func fill(_ opened: OpenedEntry, entry: EntryRef) {
        let password = opened.password()
        let user = opened.username() ?? EntrySite.fallbackUser(forPath: entry.path) ?? ""
        opened.close()
        source?.keyPort.session()?.forgetPin()
        guard let password, !password.isEmpty else {
            screen = isSuggestion
                ? .problem("\(entry.name) has no password", "Add one in paw, then try again.", backToList: false)
                : .problem("\(entry.name) has no password", "Pick another entry.", backToList: true)
            return
        }
        guard !finished else { return }
        finished = true
        screen = .filled
        recordPairing(entry)
        completion?.fill(user: user, password: password)
    }

    /// After a fill the person confirmed by name: next time this domain
    /// fills without asking. A small file, written before iOS takes the
    /// password and may end the extension. A failure only means asking
    /// again.
    private func recordPairing(_ entry: EntryRef) {
        defer { pairing = nil }
        guard let pairing, pairing.path == entry.path, let source, let pairings = source.pairings else { return }
        _ = try? pairings.recordPairing(store: source.store, entry: entry.path, service: pairing.site)
    }

    private func end(_ reason: AutoFillEnd) {
        guard !finished else { return }
        shutDown()
        completion?.end(reason)
    }

    /// The provider's view went away without a fill or a cancel from
    /// paw: swiped down, or closed by iOS. The request ends as cancelled,
    /// every key task with it, and the PIN is forgotten: a view iOS hid
    /// without ending the request would otherwise leave a sheet whose
    /// buttons do nothing. A Face ID prompt under way is left to answer
    /// (no PIN or key task is involved, and the entry it opens is wiped
    /// at once), in case the system prompt itself hid the view.
    func viewWentAway() {
        if case .faceId = screen {
            source?.keyPort.session()?.forgetPin()
            return
        }
        end(.userCanceled)
        shutDown()
    }

    /// The provider is going away: every task is let go of and the PIN
    /// forgotten. Safe to call more than once.
    func shutDown() {
        finished = true
        for id in Array(tasks.keys) {
            letGo(id)
        }
        pinEntry = ""
        source?.keyPort.session()?.forgetPin()
        #if !targetEnvironment(simulator)
        slotWatch = nil
        #endif
    }
}

extension AutoFillModel {
    /// Done, failed or cancelled: nothing changes it any more.
    fileprivate static func isFinal(_ state: KeyTaskState) -> Bool {
        switch state {
        case .done, .failed, .cancelled: true
        default: false
        }
    }
}
