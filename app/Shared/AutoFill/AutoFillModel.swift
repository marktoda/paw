import AuthenticationServices
import Foundation
import Observation

/// What the provider was asked for.
enum AutoFillRequest: Equatable, Sendable {
    /// The key icon or "Passwords…": a list for these sites (web addresses
    /// or domains, as iOS gives them).
    case list(services: [String])
    /// A suggestion above the keyboard was tapped: one entry, by path,
    /// and the site the suggestion was made for, when iOS says.
    case entry(path: String, service: String? = nil)
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
    /// Opened, and its own `url:` line names another site: `url` is that
    /// site, `site` this one.
    case urlDiffers(url: String, site: String)
}

/// What a request offers, settled once the store has opened.
private enum AutoFillOffer {
    /// The key icon or "Passwords…": every match for the site, best first,
    /// lookalikes included. Cancel and Back return to the list.
    case site([MatchedEntry])
    /// A suggestion above the keyboard: this entry, which iOS offers only
    /// for the site it was published for, so it fills without asking. The
    /// request was for this one fill: Cancel ends it.
    case suggestion(EntryRef)
}

/// What the store gives as it opens, read off the main thread.
private struct AutoFillLoad {
    let opening: AutoFillOpening
    /// Entries with a live Face ID copy. A list that can't be read marks
    /// none: each still fills, with the key.
    var live: [String] = []
    var withCode: [String] = []
    var site: String?
    /// `nil` for a suggestion whose entry is no longer in the store.
    var offer: AutoFillOffer?

    init(_ opening: AutoFillOpening, for request: AutoFillRequest) {
        self.opening = opening
        guard case .ready(let source) = opening else { return }
        live = ((try? source.copies?.list()) ?? []).map(\.entry)
        withCode = source.codes.entries(store: source.store)
        switch request {
        case .list(let services):
            site = services.lazy.compactMap { serviceHost(service: $0) }.first
            offer = .site(source.store.matchServices(services: services, pairings: source.pairings))
        case .entry(let path, _):
            offer = source.store.entry(path: path).map { .suggestion($0) }
        }
    }
}

/// A Face ID problem from the last tap, said at the top of the list.
struct AutoFillNotice: Equatable {
    let problem: String
    let nextStep: String
    /// Offers to fill this entry with the key instead.
    var keyEntry: EntryRef? = nil
}

/// The AutoFill screens' one model: the list for a site, search, and a
/// fill by Face ID copy or by the key. The same code runs in the extension
/// and, in debug builds, in the app's harness (`-pawHostProvider`), both
/// through `AutoFillProvider`.
///
/// The password never reaches a view: it goes from the opened entry
/// straight to the provider, and the entry is wiped at once. So does a
/// one-time code, made from the entry at that moment: neither the code nor
/// its secret is kept. File and card work runs off the main thread.
///
/// Which entries a list offers for a site comes from names alone: nothing
/// is decrypted to build a list or a suggestion (design.md 5.6 rule 6).
/// The core does the matching (`PassStore.matchServices`,
/// core/src/store/service.rs). A name-only entry (`bankofamerica`) fits
/// lookalikes too (`bankofamerica.co`), so the first fill on each domain
/// asks, and the domains it has filled on are kept (`SitePairings`). One
/// paired only with other domains of the same name is not offered at all.
/// iOS reports a site in its `xn--` form, and names are shown in that form
/// (`displayHost`), so a lookalike spelling shows. Search still finds every
/// entry.
@MainActor @Observable
final class AutoFillModel {
    enum Screen: Equatable {
        case loading
        case list
        /// The entry is not one of this site's, or is by name only and not
        /// yet filled here: fill it anyway, or not.
        case confirm(EntryRef, AutoFillConfirmReason)
        case faceId(EntryRef)
        case key
        case filled
        case noStore
        /// paw is using the store; Try again opens it again.
        case busy
        /// One short line and one next step, with Close; and "Back to the
        /// list" when the list is where the next step is.
        case problem(String, String, backToList: Bool)
    }

    let request: AutoFillRequest
    let kind: AutoFillKind
    /// What shows under the key sheet, and once it has gone.
    private var page: Screen = .loading
    /// The site the list is for, e.g. `github.com`. `nil` for an app iOS
    /// gives no site for.
    private(set) var site: String?
    private var offer: AutoFillOffer = .site([])
    /// Entries known to have a one-time code (`OneTimeCodeEntries`): seen
    /// with one when last opened on this iPhone.
    private(set) var withCode: Set<String> = []
    /// The name match the person confirmed for this domain, recorded once
    /// it has filled: (entry path, registrable domain).
    @ObservationIgnored private var pairing: (path: String, site: String)?
    /// The entry the person has already been warned is for another site,
    /// in this pick, and filled anyway: its `url:` line is not checked
    /// again.
    @ObservationIgnored private var warned: String?
    var query = "" {
        didSet { search() }
    }
    private(set) var hits: [SearchHit] = []
    /// Entries with a live Face ID copy.
    private var live: Set<String> = []
    private(set) var notice: AutoFillNotice?
    private(set) var keyEntry: EntryRef?
    /// The key sheet's task: opening `keyEntry` with the key. Done fills at
    /// once, with no pause: the request ends there. The PIN is forgotten
    /// whenever the sheet closes.
    let keyTask: KeyTaskDriver
    let faceId: FaceIdOpener
    /// Face ID copies can be kept and are on in Settings: the key sheet
    /// says fills afterwards use Face ID.
    private(set) var keepsCopies = false

    @ObservationIgnored private let open: @Sendable () -> AutoFillOpening
    @ObservationIgnored private var source: AutoFillSource?
    /// Where the request ends, and whether it has.
    @ObservationIgnored private weak var provider: AutoFillProvider?
    @ObservationIgnored private var started = false
    @ObservationIgnored private let work = DispatchQueue(label: "paw-autofill", qos: .userInitiated)

    init(request: AutoFillRequest, kind: AutoFillKind, provider: AutoFillProvider,
         open: @escaping @Sendable () -> AutoFillOpening) {
        self.request = request
        self.kind = kind
        self.provider = provider
        self.open = open
        keyTask = KeyTaskDriver(reader: nil)
        faceId = FaceIdOpener()
    }

    var screen: Screen {
        keyTask.isShown ? .key : page
    }

    func start() {
        guard !started else { return }
        started = true
        let open = open
        let request = request
        work.async {
            let carried = Handoff(value: AutoFillLoad(open(), for: request))
            DispatchQueue.main.async {
                MainActor.assumeIsolated { [weak self] in
                    self?.opened(carried.value)
                }
            }
        }
    }

    private func opened(_ load: AutoFillLoad) {
        withCode = Set(load.withCode)
        switch load.opening {
        case .noStore:
            page = .noStore
        case .busy:
            page = .busy
        case .unreadable:
            let words = AutoFillWords.storeUnreadable
            page = .problem(words.problem, words.nextStep, backToList: false)
        case .ready(let source):
            self.source = source
            // The key is read once a fill needs it: a list alone never
            // touches the card.
            keyTask.reader = KeyReader(port: source.keyPort, source: source.keys)
            keepsCopies = source.copies?.copiesEnabled() == true
            live = Set(load.live)
            guard let offer = load.offer else {
                end(.credentialIdentityNotFound)
                return
            }
            self.offer = offer
            site = load.site
            page = .list
            if case .suggestion(let entry) = offer {
                pick(entry)
            }
        }
    }

    /// Try again after "The store is busy".
    func reopen() {
        guard screen == .busy else { return }
        page = .loading
        started = false
        start()
    }

    // MARK: The list

    private func search() {
        guard let source else { return }
        hits = isSearching ? source.store.search(query: trimmedQuery, limit: 100) : []
    }

    private var trimmedQuery: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var isSearching: Bool {
        !trimmedQuery.isEmpty
    }

    /// The entries the request offers: the site's matches, or the
    /// suggested entry.
    private var offered: [EntryRef] {
        switch offer {
        case .site(let matches): matches.map(\.entry)
        case .suggestion(let entry): [entry]
        }
    }

    /// The site's own entries the list shows, under "For <site>": by host,
    /// or by a name already filled on this domain. Search still finds the
    /// rest.
    var listedMatches: [EntryRef] {
        offered.filter { isMatch($0) && kind.lists($0.path, knownCodes: withCode) }
    }

    /// Named like the site but never filled on this domain, listed apart
    /// under "Might be for this site". A name match paired only with other
    /// domains (a lookalike) is in neither list and is found by search
    /// only.
    var listedMaybe: [EntryRef] {
        offered.filter { isMaybe($0) && kind.lists($0.path, knownCodes: withCode) }
    }

    /// Whether the entry fills without asking.
    func isMatch(_ entry: EntryRef) -> Bool {
        confirmReason(entry) == nil
    }

    /// Whether the entry is named like the site but not yet filled here.
    func isMaybe(_ entry: EntryRef) -> Bool {
        if case .firstTime? = confirmReason(entry) { return true }
        return false
    }

    /// The site's match for `entry`, if it is one of the site's.
    private func siteMatch(_ entry: EntryRef) -> MatchedEntry? {
        guard case .site(let matches) = offer else { return nil }
        return matches.first { $0.entry.path == entry.path }
    }

    /// Why `entry` asks before it fills; `nil` if it fills at once.
    private func confirmReason(_ entry: EntryRef) -> AutoFillConfirmReason? {
        if case .suggestion = offer { return nil }
        guard let match = siteMatch(entry) else { return .otherSite }
        return Self.reason(for: match)
    }

    /// A match by host, or by a name already filled on this domain, fills
    /// at once; any other name match asks.
    private static func reason(for match: MatchedEntry) -> AutoFillConfirmReason? {
        switch match.kind {
        case .exactHost, .sameDomain, .name(.paired):
            return nil
        case .name(.unpaired):
            return .firstTime(site: displayHost(host: match.site))
        case .name(.pairedElsewhere(let known)):
            return .lookalike(site: displayHost(host: match.site), known: known.map { displayHost(host: $0) })
        }
    }

    /// Cancel and Back return to the list, except for a suggestion: its
    /// request was for one fill, so it ends.
    private var backEnds: Bool {
        if case .suggestion = offer { return true }
        return false
    }

    /// Back to the list, or the request ends (`backEnds`).
    private func back() {
        if backEnds {
            end(.userCanceled)
        } else {
            page = .list
        }
    }

    /// What a fill says it does: "Fill github.com", or with the site when
    /// the entry's name does not already say it, "Fill fastmail on
    /// example.com"; for a code, "Fill code for github.com". The key sheet
    /// and the Face ID prompt both say this.
    func purpose(_ entry: EntryRef) -> String {
        guard let site, site != entry.name.lowercased() else { return "\(kind.fillVerb) \(entry.name)" }
        return "\(kind.fillVerb) \(entry.name) on \(site)"
    }

    /// A tap on an entry. One that is not this site's asks first (design
    /// 2.10): a search on a lookalike site must not hand over the real
    /// site's password with a single Face ID glance. So does one that
    /// matches by name only, until it has filled on this domain.
    func pick(_ entry: EntryRef) {
        guard screen == .list else { return }
        notice = nil
        pairing = nil
        warned = nil
        if let reason = confirmReason(entry) {
            page = .confirm(entry, reason)
        } else {
            open(entry)
        }
    }

    /// Fill on the confirm step. A name match
    /// confirmed the first time is paired with the domain once it fills.
    /// Nothing else ever is: not a password from search for another site,
    /// not a lookalike of a domain the entry is paired with, which fills
    /// this once and warns every time (the core refuses it too), and not
    /// one whose `url:` line names another site.
    ///
    /// A first-time name match is still checked against its `url:` line
    /// once opened. The other warnings already said this is another site,
    /// so the entry fills without a second one.
    func fillAnyway() {
        guard case .confirm(let entry, let reason) = screen else { return }
        switch reason {
        case .firstTime:
            pairing = siteMatch(entry).map { (path: entry.path, site: $0.site) }
            warned = nil
        case .lookalike, .otherSite, .urlDiffers:
            pairing = nil
            warned = entry.path
        }
        page = .list
        open(entry)
    }

    /// Cancel on the confirm step, or "Back to the list" after a problem:
    /// the list again, nothing filled. A suggestion has no list: its
    /// request ends.
    func backToList() {
        pairing = nil
        warned = nil
        switch screen {
        case .confirm: back()
        case .problem(_, _, backToList: true): page = .list
        default: break
        }
    }

    /// Whether opening `entry` now takes the key rather than Face ID, as
    /// its row and the url warning say: Face ID if it has a live copy.
    func opensWithKey(_ entry: EntryRef) -> Bool {
        !(live.contains(entry.path) && source?.copies != nil)
    }

    private func open(_ entry: EntryRef) {
        if opensWithKey(entry) {
            openWithKey(entry)
        } else {
            openWithFaceId(entry)
        }
    }

    /// Cancel in the toolbar, or Close: the request ends, nothing filled.
    func cancel() {
        end(.userCanceled)
    }

    // MARK: Face ID

    private func openWithFaceId(_ entry: EntryRef) {
        guard let source, let copies = source.copies else { return }
        page = .faceId(entry)
        faceId.open(entry, store: source.store, copies: copies, reason: purpose(entry)) { [weak self] result in
            self?.faceIdFinished(entry, result)
        }
    }

    private func faceIdFinished(_ entry: EntryRef, _ result: Result<FaceIdOpen, Error>) {
        switch result {
        case .success(.opened(let opened)):
            fill(opened, entry: entry)
        case .success(.needsKey):
            live.remove(entry.path)
            page = .list
            openWithKey(entry)
        case .failure(let error):
            if case .FaceIdCancelled? = error as? PawError, backEnds {
                end(.userCanceled)
                return
            }
            page = .list
            notice = Self.faceIdNotice(error, entry: entry)
        }
    }

    private static func faceIdNotice(_ error: Error, entry: EntryRef) -> AutoFillNotice {
        let words = FaceIdWords.notice(error, entry: entry.name, table: .autoFill)
        return AutoFillNotice(problem: words.problem, nextStep: words.nextStep, keyEntry: words.offersKey ? entry : nil)
    }

    // MARK: The key

    /// Fills with the key: the sheet at the first stage still needed.
    func openWithKey(_ entry: EntryRef) {
        guard let source, let reader = keyTask.reader, !keyTask.isFollowing else { return }
        notice = nil
        keyEntry = entry
        reader.watch()
        keyTask.start(keyJob(entry, source: source))
    }

    /// Cancel on the key sheet: back to the list, or, for a suggestion
    /// (which shows nothing else), the request ends.
    func cancelKey() {
        keyTask.cancel()
        keyEntry = nil
        back()
    }

    // MARK: Ending

    /// Hands the credential over and wipes the entry. The password or code
    /// is in this function's locals only, never in a view or the
    /// pasteboard.
    ///
    /// First, the entry's own `url:` lines: if they name another site, the
    /// entry is wiped unread and the person is asked. Filling anyway opens
    /// it again (Face ID, from the copy the key just kept).
    private func fill(_ opened: OpenedEntry, entry: EntryRef) {
        guard let source else {
            opened.close()
            return
        }
        if let warning = urlWarning(opened, entry: entry) {
            opened.close()
            guard isUnderWay else { return }
            pairing = nil
            page = .confirm(entry, warning)
            return
        }
        let taken = kind.take(from: opened, entry: entry, nowMs: source.clock.wallMs())
        opened.close()
        let codes = source.codes
        guard let credential = taken.credential else {
            noteCode(taken.hasCode, entry: entry, codes: codes)
            if !taken.hasCode {
                withCode.remove(entry.path)
            }
            if let unusable = taken.unusable {
                page = .problem(unusable.problem, unusable.nextStep, backToList: !backEnds)
            } else {
                let words = AutoFillWords.nothingToFill(entry.name, kind, fromList: !backEnds)
                page = .problem(words.problem, words.nextStep, backToList: !backEnds)
            }
            return
        }
        guard isUnderWay else { return }
        page = .filled
        recordPairing(entry)
        provider?.fill(credential, for: self)
        noteCode(taken.hasCode, entry: entry, codes: codes)
    }

    /// Keeps whether the entry just opened has a one-time code, so a code
    /// request lists it next time: after the fill is handed over, and only
    /// when that is not what is kept already. A yes per entry, never the
    /// code or its secret; nothing while the list is off (the core's
    /// switch); a failure only leaves it as it was.
    private func noteCode(_ hasCode: Bool, entry: EntryRef, codes: OneTimeCodeEntries) {
        guard hasCode != withCode.contains(entry.path) else { return }
        _ = try? codes.note(entry: entry.path, hasCode: hasCode)
    }

    /// The warning to show before filling `entry` on this site, from its
    /// `url:` lines (`checkEntryUrl` in the core), unless the person was
    /// already told this is another site.
    private func urlWarning(_ opened: OpenedEntry, entry: EntryRef) -> AutoFillConfirmReason? {
        guard warned != entry.path else { return nil }
        let services: [String]
        switch request {
        case .list(let all): services = all
        case .entry(_, let service): services = service.map { [$0] } ?? []
        }
        switch checkEntryUrl(entry: opened, services: services) {
        case .differs(let url, let site): return .urlDiffers(url: url, site: site)
        case .noUrl, .same: return nil
        }
    }

    /// After a fill the person confirmed by name: next time this domain
    /// fills without asking. A small file, written before iOS takes the
    /// password and may end the extension. A failure only means asking
    /// again.
    private func recordPairing(_ entry: EntryRef) {
        defer { pairing = nil }
        guard let pairing, pairing.path == entry.path, let source else { return }
        _ = try? source.pairings.recordPairing(store: source.store, entry: entry.path, service: pairing.site)
    }

    private var isUnderWay: Bool {
        provider?.model === self
    }

    private func end(_ reason: ASExtensionError.Code) {
        provider?.end(reason, for: self)
    }

    /// The provider's view went away without a fill or a cancel from
    /// paw: swiped down, or closed by iOS. The request ends as cancelled,
    /// every key task with it, and the PIN is forgotten: a view iOS hid
    /// without ending the request would otherwise leave a sheet whose
    /// buttons do nothing. A Face ID prompt under way is left to answer
    /// (no PIN or key task is involved, and the entry it opens is wiped
    /// at once), in case the system prompt itself hid the view.
    func viewWentAway() {
        if case .faceId = page {
            return
        }
        end(.userCanceled)
    }

    /// The request's one end, however it ends (a fill, `end`, the view or
    /// the controller going away), run by the provider once: every key
    /// task is let go of, the PIN is forgotten (the key sheet closing
    /// forgets it too), a Face ID answer still to come is wiped unread,
    /// and the readers are no longer followed.
    func shutDown() {
        keyTask.cancelAll()
        source?.keyPort.session()?.forgetPin()
        faceId.forget()
        keyTask.reader?.stopWatching()
    }
}

extension AutoFillModel {
    /// Opening `entry` with the key to fill it. A copy is kept after the
    /// key opens the entry, as in the app: the copy cache is safe for two
    /// processes (core/src/copies/dir.rs).
    private func keyJob(_ entry: EntryRef, source: AutoFillSource) -> KeyJob<Never> {
        let copies = source.copies
        let port = source.keyPort
        let open = KeyOpen(entry: entry, store: source.store, port: port, copies: copies, table: .autoFill)
        var job = open.job(
            opened: { [weak self] opened in
                // A copy that wasn't kept goes unsaid: the request ends
                // with the fill, and the next fill uses the key again.
                if opened.faceIdUntilMs != nil {
                    self?.live.insert(entry.path)
                }
                self?.fill(opened.entry, entry: entry)
                // A fill shows no Done: the request ends there.
                return .close
            },
            lateCopyKept: { [weak self] in
                guard let copies else { return }
                let carried = Handoff(value: copies)
                self?.work.async { try? carried.value.forgetEntry(entry: entry.path) }
            })
        job.closed = {
            port.session()?.forgetPin()
        }
        return job
    }
}
