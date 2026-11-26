import AuthenticationServices
import Foundation

/// The suggestions above the keyboard: what paw tells
/// `ASCredentialIdentityStore`, from the app only (design.md 2.10).
///
/// One identity per entry whose path names a site (`web/github.com`,
/// `github.com/alice`): the site as a domain, the entry's name as the user
/// string, the path as the record. An entry named without a domain
/// (`bankofamerica`) gets one only for each domain it has filled on after
/// the person confirmed it (`SitePairings`): never for a domain merely
/// named like it. Nothing learned by decrypting is used
/// (design.md 5.6 rule 6), with one exception the person turns on in
/// Settings: an entry seen with a one-time code when last opened
/// (`OneTimeCodeEntries`, a yes per entry) also gets a one-time-code
/// identity for each of its sites, so iOS offers it in a code field. Off,
/// the core lists none. The list is replaced whole after setup, on each
/// launch, after a save, rename, delete or refresh (`AutoFillSuggestions`);
/// removing the store clears it. App target only (not compiled into the
/// extension).
enum CredentialIdentities {
    struct Identity: Equatable, Sendable {
        let domain: String
        let user: String
        let path: String
    }

    /// More than this is a store paw cannot sensibly suggest from anyway.
    static let limit = 5_000

    /// Every entry with a site in its path, sorted by path. A site that is
    /// itself a public suffix (`co.uk`, `github.io`) gives none: iOS might
    /// offer it on every site under that suffix.
    static func make(from store: PassStore, pairings: SitePairings? = nil) -> [Identity] {
        var identities: [Identity] = []
        for entry in store.allEntries() where identities.count < limit {
            if let domain = suggestionDomain(path: entry.path) {
                identities.append(Identity(domain: domain, user: entry.name, path: entry.path))
            }
        }
        var seen = Set(identities.map { "\($0.domain) \($0.path)" })
        for paired in pairings?.pairedSites(store: store) ?? [] where identities.count < limit {
            guard seen.insert("\(paired.site) \(paired.entry)").inserted else { continue }
            identities.append(Identity(
                domain: paired.site, user: entryRef(path: paired.entry).name, path: paired.entry))
        }
        return Array(identities.sorted { ($0.path, $0.domain) < ($1.path, $1.domain) }.prefix(limit))
    }

    /// What iOS should end up with: a store's identities, or none.
    enum Request: @unchecked Sendable {
        /// The store's identities, with the pairings and the entries with a
        /// one-time code kept for it.
        case replace(PassStore, pairings: SitePairings, codes: OneTimeCodeEntries)
        case clear
    }

    /// The newest request not yet started. A newer one replaces it, so
    /// requests made while a write runs collapse into one: the last.
    @MainActor private static var pending: Request?
    /// The one writer, while it runs. Writes never overlap, so an older
    /// list can never land after a newer one (two quick removes in Sites
    /// used must leave the second's list, not the first's). iOS keeps one
    /// list for the whole app, so there is one writer for it.
    @MainActor private static var writer: Task<Void, Never>?

    /// Queues `request` and returns the writer that will carry it out.
    @MainActor
    static func submit(_ request: Request) -> Task<Void, Never> {
        pending = request
        if let writer { return writer }
        let task = Task.detached(priority: .utility) { await drain() }
        writer = task
        return task
    }

    /// Carries out the newest request until none is left. Taking a request
    /// and ending the writer happen together on the main actor, so a
    /// request is never left behind.
    private static func drain() async {
        while true {
            let next: Request? = await MainActor.run {
                let taken = pending
                pending = nil
                if taken == nil { writer = nil }
                return taken
            }
            guard let next else { return }
            switch next {
            case .replace(let store, let pairings, let codes): await replace(with: store, pairings: pairings, codes: codes)
            case .clear: await removeAll()
            }
        }
    }

    /// Replaces every identity with this store's. Does nothing while
    /// AutoFill is off for paw (iOS refuses the write).
    private static func replace(with store: PassStore, pairings: SitePairings, codes: OneTimeCodeEntries) async {
        let made = make(from: store, pairings: pairings)
        let withCode = Set(codes.entries(store: store))
        let identities = AutoFillKind.allCases.flatMap { kind in
            made.filter { kind.lists($0.path, knownCodes: withCode) }
                .map { kind.identity(domain: $0.domain, user: $0.user, path: $0.path) }
        }
        #if !targetEnvironment(simulator)
        let identityStore = ASCredentialIdentityStore.shared
        guard await identityStore.state().isEnabled else { return }
        try? await identityStore.replaceCredentialIdentities(identities)
        #else
        _ = identities
        #endif
    }

    /// The store was removed: no suggestion may name its entries.
    @MainActor
    static func clear() {
        _ = submit(.clear)
    }

    private static func removeAll() async {
        #if !targetEnvironment(simulator)
        try? await ASCredentialIdentityStore.shared.removeAllCredentialIdentities()
        #endif
    }
}

/// AutoFill's suggestions for the store paw set up, from its entries and
/// what its state folder keeps (pairings, entries with a one-time code).
/// The one way they are published: when the store opens, when paw comes
/// forward and whenever the store changes (`follow`), and after a
/// background check brought changes in (`BackgroundRefresh`). Settings
/// changes what it keeps here, and it publishes again after each change.
/// A fixture's submits nothing: its entries must not reach the system's
/// suggestions.
@MainActor
final class AutoFillSuggestions {
    private let pairings: SitePairings
    private let codes: OneTimeCodeEntries
    private let submit: @MainActor (CredentialIdentities.Request) -> Task<Void, Never>
    /// The store last published, to publish again.
    private var store: PassStore?

    init(stateDir: String,
         submit: @escaping @MainActor (CredentialIdentities.Request) -> Task<Void, Never> = CredentialIdentities.submit) {
        pairings = SitePairings.open(stateDir: stateDir)
        codes = OneTimeCodeEntries.open(stateDir: stateDir)
        self.submit = submit
    }

    /// Replaces the suggestions with `store`'s. The task ends once iOS has
    /// them (or newer ones): a background task must not end before.
    @discardableResult
    func publish(_ store: PassStore) -> Task<Void, Never> {
        self.store = store
        return submit(.replace(store, pairings: pairings, codes: codes))
    }

    /// Writes the suggestions again: on coming forward, and after a
    /// setting that changes them.
    func republish() {
        if let store {
            publish(store)
        }
    }

    /// Settings › Suggest one-time codes.
    var codesEnabled: Bool {
        codes.enabled()
    }

    /// Off deletes the list of entries with a code, and their suggestions.
    func setCodesEnabled(_ on: Bool) throws {
        defer { republish() }
        try codes.setEnabled(enabled: on)
    }

    /// Settings › Sites used: each entry named without a domain and the
    /// sites it was filled on, by entry.
    var sites: [Pairing] {
        pairings.pairings().sorted { $0.entry < $1.entry }
    }

    /// The next fill of `entry` on `site` asks again.
    func forgetSite(entry: String, site: String) throws {
        defer { republish() }
        try pairings.forgetSite(entry: entry, site: site)
    }

    /// The next fill on every site asks again.
    func forgetAllSites() throws {
        defer { republish() }
        try pairings.clearPairings(entry: nil)
    }

    /// Publishes `live`'s store now and whenever its entries change, and
    /// keeps which opened entries have a one-time code.
    func follow(_ live: LiveStore) {
        live.changed.subscribe(self) { suggestions, changed in
            if changed.change.rewritesEntries {
                suggestions.publish(changed.store)
            }
        }
        live.opened.subscribe(self) { suggestions, opened in
            suggestions.noteOpened(opened)
        }
        publish(live.store)
    }

    /// Keeps whether the entry has a one-time code, off the main thread,
    /// and writes the suggestions again when that changed, so its code
    /// suggestion appears or goes. Nothing while the list is off.
    private func noteOpened(_ opened: EntryOpened) {
        let hasCode = opened.entry.makesOneTimeCodes()
        let codes = codes
        let path = opened.path
        Task.detached(priority: .utility) { [weak self] in
            guard (try? codes.note(entry: path, hasCode: hasCode)) == true else { return }
            await MainActor.run {
                self?.republish()
            }
        }
    }
}
