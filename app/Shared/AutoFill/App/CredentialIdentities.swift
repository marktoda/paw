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
/// (design.md 5.6 rule 6). The list is replaced whole after setup, on each
/// launch, after a save, rename, delete or refresh; removing the store
/// clears it. App target only (not compiled into the extension).
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
        var registrable: [String: Bool] = [:]
        var folders = [store.home()]
        while let listing = folders.popLast(), identities.count < limit {
            for entry in listing.entries {
                if let site = EntrySite.domain(ofPath: entry.path),
                   isRegistrable(site.domain, in: store, known: &registrable) {
                    identities.append(Identity(
                        domain: site.domain, user: EntrySite.suggestionUser(forPath: entry.path), path: entry.path))
                }
            }
            for folder in listing.folders {
                if let inner = try? store.folder(path: folder.path) {
                    folders.append(inner)
                }
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

    /// Whether `domain` is a registrable domain or a name under one, by the
    /// core's Public Suffix List: a made-up name one label below it shares
    /// its registrable domain with `domain` exactly when `domain` is not a
    /// public suffix, so the core then matches the entry that names it.
    private static func isRegistrable(_ domain: String, in store: PassStore, known: inout [String: Bool]) -> Bool {
        if let answer = known[domain] { return answer }
        let answer = !store.matchService(service: "https://paw-suffix-probe.\(domain)").isEmpty
        known[domain] = answer
        return answer
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
        let identities = make(from: store, pairings: standardPairings()).map { identity -> ASCredentialIdentity in
            ASPasswordCredentialIdentity(
                serviceIdentifier: ASCredentialServiceIdentifier(identifier: identity.domain, type: .domain),
                user: identity.user, recordIdentifier: identity.path)
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
