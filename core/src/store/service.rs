//! Which entries belong to a website or app, for AutoFill.
//!
//! The only evidence is the entry's path: `web/github.com`,
//! `github.com/alice` and `web/github.com/alice` all name the host in one
//! part of the path. An entry is offered when one part of its path
//!
//! 1. is the host itself, or
//! 2. is a host with the same registrable domain (`github.com` for
//!    `accounts.github.com`), by the Public Suffix List, so that
//!    `alice.github.io` and `bob.github.io` stay strangers.
//!
//! Nothing is ever matched on a piece of a name. `hub.com` is not
//! `github.com` and `github.com.evil.example` is not `github.com`.
//!
//! Hosts are compared as lower-case text. An internationalised name matches
//! only if the entry spells it the same way the system reports it.
//!
//! **Name matches** ([`match_site`]). Many stores name entries without a
//! top-level domain: `bankofamerica`, `bank/bankofamerica`,
//! `alaskaair/alice`. Such an entry matches a site when one eligible part
//! of its path, with letter case, spaces, `-` and `_` taken out, equals the
//! first label of the site's registrable domain, treated the same way
//! ([`site_name`]): `bankofamerica` for `www.bankofamerica.com`, `barclays`
//! for `www.barclays.co.uk`, `alice` (not `github`) for `alice.github.io`.
//! Whole parts only, at least three letters or digits, plain ASCII, never a
//! `xn--` label, never an IP address or a dotless host.
//!
//! Eligible parts: every part without a dot, except the entry's own name
//! when an earlier part is a host (`github.com/bankofamerica` is the user
//! `bankofamerica` on github.com, as the user name is derived in the app).
//! Folders count, so `bankofamerica/alice` matches; so does a category
//! folder named like a site (`bank/chase` on `bank.com`). Nothing is capped:
//! a folder named like a site is offered there, behind the first-use
//! confirm.
//!
//! A name carries no domain, so `bankofamerica.co` has the same name as
//! `bankofamerica.com`. Every name match is therefore classed by the
//! domains the person has confirmed it on ([`super::pairings`]): paired
//! with this domain, not yet paired, or paired only with other domains of
//! the same name (a lookalike: the app does not offer it).

use super::pairings::Pairing;
use super::tree::EntryRef;

/// An entry that may hold the login for a service.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ServiceMatch {
    pub entry: EntryRef,
    /// A part of the path is exactly the host asked for. Otherwise it is
    /// another host of the same registrable domain.
    pub exact: bool,
}

fn is_host(text: &str) -> bool {
    !text.is_empty()
        && text.split('.').all(|label| {
            !label.is_empty()
                && label.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        })
}

/// The host a service identifier names: `accounts.github.com` for both
/// `accounts.github.com` and `https://user@Accounts.GitHub.com:443/login`.
/// `None` if there is no usable host in it.
pub fn host_of(service: &str) -> Option<String> {
    let service = service.trim();
    // Only a real scheme at the very start is a scheme. A "://" later on is
    // part of a path or query (`evil.example/?next=://github.com`) and must
    // not move where the host is read from.
    let rest = match service.split_once("://") {
        Some((scheme, rest)) if is_scheme(scheme) => rest,
        _ => service,
    };
    // Browsers read `\` as `/` in web addresses, so it ends the host too.
    let authority = rest.split(['/', '\\', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    };
    let host = host.strip_suffix('.').unwrap_or(host).to_lowercase();
    is_host(&host).then_some(host)
}

/// Whether `text` is a URL scheme: a letter, then letters, digits, `+`, `-`
/// or `.`.
fn is_scheme(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// The registrable domain of `host`, e.g. `example.co.uk` for
/// `www.example.co.uk`. `None` for a bare word and for an IP address, which
/// only ever match exactly.
pub(crate) fn registrable(host: &str) -> Option<&str> {
    let last = host.rsplit('.').next()?;
    if last.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    psl::domain_str(host)
}

/// The shortest name that may match a site.
pub const MIN_NAME_LEN: usize = 3;

/// `text` in lower case without spaces, `-` and `_`, if what is left is at
/// least [`MIN_NAME_LEN`] ASCII letters and digits.
fn squeeze(text: &str) -> Option<String> {
    let key: String = text
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .map(|c| c.to_ascii_lowercase())
        .collect();
    (key.len() >= MIN_NAME_LEN && key.chars().all(|c| c.is_ascii_alphanumeric())).then_some(key)
}

/// The name a path part gives for name matching: `bankofamerica` for
/// `BankOfAmerica`, `Bank of America` and `bank-of-america`. `None` for a
/// part with a dot (a host, matched by host) or any other character.
pub fn name_key(part: &str) -> Option<String> {
    if part.contains('.') {
        return None;
    }
    squeeze(part)
}

/// The name of the site `host` belongs to: the first label of its
/// registrable domain, squeezed like [`name_key`]. `None` for a dotless
/// host, an IP address, a public suffix, a `xn--` label or one shorter than
/// [`MIN_NAME_LEN`].
pub fn site_name(host: &str) -> Option<String> {
    if !host.contains('.') || !host.is_ascii() || !is_host(host) {
        return None;
    }
    let label = registrable(host)?.split('.').next()?;
    if label.starts_with("xn--") {
        return None;
    }
    squeeze(label)
}

/// The registrable domain a service identifier is on, for pairing:
/// `bankofamerica.com` for `https://www.bankofamerica.com/login`. `None`
/// where there is none.
pub fn registrable_domain(service: &str) -> Option<String> {
    let host = host_of(service)?;
    registrable(&host).map(str::to_owned)
}

/// Whether `path` matches the site named `name` by name ([`site_name`]).
pub fn names_site(path: &str, name: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let Some((last, folders)) = parts.split_last() else { return false };
    let user_layout = folders.iter().any(|part| part.contains('.') && is_host(&part.to_lowercase()));
    folders
        .iter()
        .chain((!user_layout).then_some(last))
        .any(|part| name_key(part).as_deref() == Some(name))
}

/// How well one part of an entry's path fits the host: 0 exact, 1 same
/// registrable domain.
fn fit(part: &str, host: &str, host_domain: Option<&str>) -> Option<u8> {
    let part = part.to_lowercase();
    if part == host {
        return Some(0);
    }
    // A part without a dot is a word, not a host.
    if !part.contains('.') || !is_host(&part) {
        return None;
    }
    let domain = registrable(&part)?;
    (Some(domain) == host_domain).then_some(1)
}

/// The entries to offer for `service` (a host name or a web address), best
/// first: exact hosts, then the same registrable domain; within each, the
/// shorter path, then alphabetically.
pub fn match_service(entries: &[EntryRef], service: &str) -> Vec<ServiceMatch> {
    let Some(host) = host_of(service) else {
        return Vec::new();
    };
    let host_domain = registrable(&host);

    let mut matches: Vec<(u8, &EntryRef)> = entries
        .iter()
        .filter_map(|entry| {
            let best = entry.path.split('/').filter_map(|part| fit(part, &host, host_domain)).min()?;
            Some((best, entry))
        })
        .collect();
    matches.sort_by(|a, b| {
        (a.0, a.1.path.chars().count(), a.1.path.to_lowercase(), &a.1.path).cmp(&(
            b.0,
            b.1.path.chars().count(),
            b.1.path.to_lowercase(),
            &b.1.path,
        ))
    });
    matches
        .into_iter()
        .map(|(fit, entry)| ServiceMatch { entry: entry.clone(), exact: fit == 0 })
        .collect()
}

/// Whether a name match is one the person has confirmed on this site.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum NamePairing {
    /// Filled on this registrable domain before: no question asked.
    Paired,
    /// Never filled on a site of this name: ask first.
    Unpaired,
    /// Filled only on other domains of the same name (`bankofamerica.com`,
    /// now on `bankofamerica.co`): a lookalike until the person says not.
    PairedElsewhere { sites: Vec<String> },
}

/// Why an entry is offered for a site.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum MatchKind {
    /// A part of the path is the host.
    ExactHost,
    /// A part of the path is a host of the same registrable domain.
    SameDomain,
    /// A part of the path is the site's name, without a domain.
    Name { pairing: NamePairing },
}

/// An entry offered for a site, and why.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MatchedEntry {
    pub entry: EntryRef,
    pub kind: MatchKind,
    /// The site's registrable domain (`bankofamerica.com`), the domain a
    /// name match is paired with. The host for an address.
    pub site: String,
}

fn rank(kind: &MatchKind) -> u8 {
    match kind {
        MatchKind::ExactHost => 0,
        MatchKind::SameDomain => 1,
        MatchKind::Name { pairing: NamePairing::Paired } => 2,
        MatchKind::Name { pairing: NamePairing::Unpaired } => 3,
        MatchKind::Name { pairing: NamePairing::PairedElsewhere { .. } } => 4,
    }
}

/// Every entry that matches `service` by host ([`match_service`]) or by
/// name, best first: exact host, same registrable domain, then name
/// matches paired here, not yet paired, and paired only elsewhere. Within
/// each, the shorter path, then alphabetically. `pairings` are the
/// confirmed pairings ([`super::pairings`]).
pub fn match_site(entries: &[EntryRef], service: &str, pairings: &[Pairing]) -> Vec<MatchedEntry> {
    let Some(host) = host_of(service) else {
        return Vec::new();
    };
    let host_domain = registrable(&host);
    let site = host_domain.unwrap_or(&host).to_owned();
    let name = site_name(&host);

    let mut matches: Vec<MatchedEntry> = entries
        .iter()
        .filter_map(|entry| {
            let best = entry.path.split('/').filter_map(|part| fit(part, &host, host_domain)).min();
            let kind = match best {
                Some(0) => MatchKind::ExactHost,
                Some(_) => MatchKind::SameDomain,
                None => {
                    let name = name.as_deref()?;
                    if !names_site(&entry.path, name) {
                        return None;
                    }
                    MatchKind::Name { pairing: pairing_of(pairings, &entry.path, &site, name) }
                }
            };
            Some(MatchedEntry { entry: entry.clone(), kind, site: site.clone() })
        })
        .collect();
    matches.sort_by(|a, b| {
        let key = |m: &MatchedEntry| (rank(&m.kind), m.entry.path.chars().count(), m.entry.path.to_lowercase());
        key(a).cmp(&key(b)).then_with(|| a.entry.path.cmp(&b.entry.path))
    });
    matches
}

/// How `path`, a name match for the site `site` named `name`, stands with
/// the pairings. Only pairings with a domain of the same name count as
/// "elsewhere": a pairing for another name does not make this a lookalike.
fn pairing_of(pairings: &[Pairing], path: &str, site: &str, name: &str) -> NamePairing {
    let Some(known) = pairings.iter().find(|p| p.entry == path) else {
        return NamePairing::Unpaired;
    };
    if known.sites.iter().any(|s| s == site) {
        return NamePairing::Paired;
    }
    let sites: Vec<String> =
        known.sites.iter().filter(|s| site_name(s).as_deref() == Some(name)).cloned().collect();
    if sites.is_empty() { NamePairing::Unpaired } else { NamePairing::PairedElsewhere { sites } }
}
