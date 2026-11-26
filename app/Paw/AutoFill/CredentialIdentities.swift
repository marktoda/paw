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
/// launch, after a save, rename, delete or refresh (`AutoFillUpkeep`);
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
                domain: paired.site, user: EntrySite.suggestionUser(forPath: paired.entry), path: paired.entry))
        }
        return Array(identities.sorted { ($0.path, $0.domain) < ($1.path, $1.domain) }.prefix(limit))
    }

    /// The pairings of the store paw set up, if there is one.
    static func standardPairings() -> SitePairings? {
        (try? StoreLocation.standard()).map { SitePairings.open(stateDir: $0.stateDir.path) }
    }

    /// Which entries of the store paw set up have a one-time code.
    static func standardCodes() -> OneTimeCodeEntries? {
        (try? StoreLocation.standard()).map { OneTimeCodeEntries.open(stateDir: $0.stateDir.path) }
    }

    /// The store the last publish used, for Settings to publish again once
    /// AutoFill is turned on.
    @MainActor private(set) static var current: PassStore?

    /// What iOS should end up with: this store's identities, or none.
    private enum Request: @unchecked Sendable {
        case replace(PassStore)
        case clear
    }

    /// The newest request not yet started. A newer one replaces it, so
    /// requests made while a write runs collapse into one: the last.
    @MainActor private static var pending: Request?
    /// The one writer, while it runs. Writes never overlap, so an older
    /// list can never land after a newer one (two quick removes in Sites
    /// used must leave the second's list, not the first's).
    @MainActor private static var writer: Task<Void, Never>?

    /// Queues `request` and returns the writer that will carry it out.
    @MainActor
    private static func submit(_ request: Request) -> Task<Void, Never> {
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
            case .replace(let store): await replace(with: store)
            case .clear: await removeAll()
            }
        }
    }

    /// Replaces every identity with this store's, off the main thread.
    /// Does nothing while AutoFill is off for paw (iOS refuses the write).
    @MainActor
    static func publish(_ store: PassStore) {
        current = store
        _ = submit(.replace(store))
    }

    /// `publish`, returning once iOS has the new identities (or newer
    /// ones): a background task must not end before.
    @MainActor
    static func publishAndWait(_ store: PassStore) async {
        current = store
        await submit(.replace(store)).value
    }

    private static func replace(with store: PassStore) async {
        let made = make(from: store, pairings: standardPairings())
        let withCode = Set(standardCodes()?.entries(store: store) ?? [])
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
        current = nil
        _ = submit(.clear)
    }

    private static func removeAll() async {
        #if !targetEnvironment(simulator)
        try? await ASCredentialIdentityStore.shared.removeAllCredentialIdentities()
        #endif
    }
}

/// Keeps AutoFill's suggestions in step with the store paw set up: writes
/// them when the store opens, when paw comes forward and whenever the store
/// changes, and keeps which opened entries have a one-time code. Built for
/// the set-up store only, never for a fixture, whose entries must not reach
/// the system's suggestions.
@MainActor
final class AutoFillUpkeep {
    private let live: LiveStore
    private let codes: OneTimeCodeEntries
    private let publish: @MainActor (PassStore) -> Void

    init(live: LiveStore, codes: OneTimeCodeEntries,
         publish: @escaping @MainActor (PassStore) -> Void = { CredentialIdentities.publish($0) }) {
        self.live = live
        self.codes = codes
        self.publish = publish
        live.changed.subscribe(self) { upkeep, changed in
            if changed.change.rewritesEntries {
                upkeep.publish(changed.store)
            }
        }
        live.opened.subscribe(self) { upkeep, opened in
            upkeep.noteOpened(opened)
        }
        publish(live.store)
    }

    /// Writes the suggestions again: on coming forward, and after a
    /// setting that changes them.
    func republish() {
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
