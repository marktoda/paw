import Foundation

/// Which entries AutoFill offers for a site, from names alone: nothing is
/// decrypted to build a list or a suggestion (design.md 5.6 rule 6).
///
/// The core does the matching (`PassStore.matchServices`,
/// core/src/store/service.rs): an entry is offered when one part of its
/// path is the site's host exactly, or a host with the same registrable
/// domain by the Public Suffix List. Never a piece of a name, so
/// `evil-example.com`, `example.com.evil.net` and `bob.github.io` are not
/// `example.com`, `example.com` and `alice.github.io`.
///
/// An entry named without a domain (`bankofamerica`, `alaskaair/alice`)
/// matches by name: a whole part of its path is the first label of the
/// site's registrable domain. A name fits lookalikes too
/// (`bankofamerica.co`), so the first fill on each domain asks, and the
/// domains it has filled on are kept (`SitePairings`). One paired only
/// with other domains of the same name is not offered at all.
///
/// Internationalised names: iOS reports a site in its `xn--` form, and the
/// core compares hosts in that form on both sides (UTS-46), so the entries
/// `bücher.example` and `xn--bcher-kva.example` both match it, and a site
/// spelled with lookalike letters (`pаypal.com`, Cyrillic `а`) never
/// matches an entry spelled with plain ones. Names are shown by the core's
/// `displayHost`: always the `xn--` form. Name
/// matches stay plain ASCII. paw offers nothing automatically for a site
/// address that is not printable ASCII, or whose host has no dot. Search
/// still finds every entry.
enum SiteMatch {
    /// The entry at `path`, if the store has it.
    static func entry(_ path: String, in store: PassStore) -> EntryRef? {
        store.entry(path: path)
    }
}

/// What an entry's path says about the site it is for, for the suggestions
/// above the keyboard. Path-derived only; the site itself is the core's
/// `suggestionDomain`.
enum EntrySite {
    /// The user string shown in a suggestion: the entry's name, which is
    /// the user in a `domain/user` layout and the site otherwise (a blank
    /// suggestion looks broken).
    static func suggestionUser(forPath path: String) -> String {
        entryRef(path: path).name
    }
}
