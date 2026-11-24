import Foundation

/// Which entries AutoFill offers for a site, from names alone: nothing is
/// decrypted to build a list or a suggestion (design.md 5.6 rule 6).
///
/// The core does the matching (`PassStore.matchSite`,
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
/// On top of that, paw offers nothing automatically for a site whose
/// address is not plain ASCII. iOS reports internationalised names in
/// their `xn--` form, the core compares names as text (no IDNA mapping),
/// and so a site spelled with lookalike letters never matches an entry
/// spelled with plain ones. Search still finds every entry.
enum SiteMatch {
    /// The host to show for a service: `github.com` for
    /// `https://github.com/login`. `nil` if there is none.
    static func host(of service: String) -> String? {
        let trimmed = service.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        let host: String
        if trimmed.contains("://") {
            guard let found = URLComponents(string: trimmed)?.host, !found.isEmpty else { return nil }
            host = found
        } else {
            host = String(trimmed.prefix { $0 != "/" && $0 != ":" })
        }
        let lowered = host.lowercased()
        let bare = lowered.hasSuffix(".") ? String(lowered.dropLast()) : lowered
        return bare.isEmpty ? nil : bare
    }

    /// Whether a service may be matched automatically at all: printable
    /// ASCII only, and a host with a dot. The core matches a dotless host
    /// (`http://github/`, an intranet name anyone can register) exactly
    /// against an entry called `GitHub`; paw does not offer that.
    static func isPlain(_ service: String) -> Bool {
        guard !service.isEmpty, service.unicodeScalars.allSatisfy({ $0.value >= 0x21 && $0.value < 0x7F }) else {
            return false
        }
        return host(of: service)?.contains(".") == true
    }

    /// Every entry that matches these services, best first: exact hosts,
    /// the same registrable domain, then name matches paired here, not yet
    /// paired and paired only elsewhere. Each entry once, as its best match.
    static func entries(in store: PassStore, for services: [String], pairings: SitePairings?) -> [MatchedEntry] {
        var best: [String: (order: Int, match: MatchedEntry)] = [:]
        var order = 0
        for service in services where isPlain(service) {
            for match in store.matchSite(service: service, pairings: pairings) {
                order += 1
                if let known = best[match.entry.path], rank(known.match.kind) <= rank(match.kind) { continue }
                best[match.entry.path] = (order, match)
            }
        }
        return best.values
            .sorted { (rank($0.match.kind), $0.order) < (rank($1.match.kind), $1.order) }
            .map(\.match)
    }

    static func rank(_ kind: MatchKind) -> Int {
        switch kind {
        case .exactHost: 0
        case .sameDomain: 1
        case .name(.paired): 2
        case .name(.unpaired): 3
        case .name(.pairedElsewhere): 4
        }
    }

    /// Whether a match fills without asking: by host, or by a name already
    /// filled on this domain.
    static func isTrusted(_ kind: MatchKind) -> Bool {
        rank(kind) <= 2
    }

    /// The entry at `path`, if the store has it.
    static func entry(_ path: String, in store: PassStore) -> EntryRef? {
        let folder = path.lastIndex(of: "/").map { String(path[..<$0]) } ?? ""
        let listing = folder.isEmpty ? store.home() : (try? store.folder(path: folder))
        return listing?.entries.first { $0.path == path }
    }
}

/// What an entry's path says about the site it is for, for the suggestions
/// above the keyboard. Path-derived only.
enum EntrySite {
    /// The site an entry is for: the last part of its path that is a plain
    /// ASCII host name with a dot (`web/github.com`, `github.com/alice`),
    /// and whether that part is the entry's own name. IP addresses and
    /// names like `GitHub` or `Zürich Tourism` give none.
    static func domain(ofPath path: String) -> (domain: String, isName: Bool)? {
        let parts = path.split(separator: "/").map(String.init)
        for (index, part) in parts.enumerated().reversed() where isPlainHost(part) {
            return (part.lowercased(), index == parts.count - 1)
        }
        return nil
    }

    /// The user string shown in a suggestion: the entry's name, which is
    /// the user in a `domain/user` layout and the site otherwise (a blank
    /// suggestion looks broken).
    static func suggestionUser(forPath path: String) -> String {
        path.split(separator: "/").last.map(String.init) ?? path
    }

    /// The user name to fill when the entry has no `login:` (or `user:`,
    /// `username:`, `email:`) line: the file name in a `domain/user` layout
    /// such as `github.com/alice`, else none.
    static func fallbackUser(forPath path: String) -> String? {
        guard let site = domain(ofPath: path), !site.isName else { return nil }
        return path.split(separator: "/").last.map(String.init)
    }

    static func isPlainHost(_ part: String) -> Bool {
        guard part.count <= 253, !part.hasPrefix("."), !part.hasSuffix(".") else { return false }
        let labels = part.lowercased().split(separator: ".", omittingEmptySubsequences: false)
        guard labels.count >= 2 else { return false }
        for label in labels {
            guard !label.isEmpty, label.count <= 63, label.first != "-", label.last != "-",
                  label.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-") }) else { return false }
        }
        // A last label with no letter is an IP address or a number.
        return labels.last?.contains(where: { $0.isLetter }) == true
    }
}
