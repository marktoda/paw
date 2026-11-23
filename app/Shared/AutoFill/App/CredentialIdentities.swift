import AuthenticationServices
import Foundation

/// The suggestions above the keyboard: what paw tells
/// `ASCredentialIdentityStore`, from the app only (design.md 2.10).
///
/// One identity per entry whose path names a site (`web/github.com`,
/// `github.com/alice`): the site as a domain, the entry's name as the user
/// string, the path as the record. Nothing learned by decrypting is used
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
    static func make(from store: PassStore) -> [Identity] {
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
        return Array(identities.sorted { $0.path < $1.path }.prefix(limit))
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

    /// Replaces every identity with this store's, off the main thread.
    /// Does nothing while AutoFill is off for paw (iOS refuses the write).
    @MainActor
    static func publish(_ store: PassStore) {
        current = store
        Task.detached(priority: .utility) {
            let identities = make(from: store).map { identity -> ASCredentialIdentity in
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
    }

    /// The store was removed: no suggestion may name its entries.
    @MainActor
    static func clear() {
        current = nil
        #if !targetEnvironment(simulator)
        Task.detached(priority: .utility) {
            try? await ASCredentialIdentityStore.shared.removeAllCredentialIdentities()
        }
        #endif
    }
}
