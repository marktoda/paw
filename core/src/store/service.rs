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
//! Hosts are compared in ASCII: lower case, and an internationalised name
//! in its `xn--` form by UTS-46 ([`ascii_host`]), on both sides. So the
//! entries `bücher.example` and `xn--bcher-kva.example` both match
//! `https://xn--bcher-kva.example`, and `pаypal.com` spelled with a
//! Cyrillic `а` is `xn--pypal-4ve.com`, a stranger to `paypal.com`.
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
//! A name carries no domain, so `bankofamerica.co` has the same name as
//! `bankofamerica.com`. Every name match is therefore classed by the
//! domains the person has confirmed it on ([`super::pairings`]): paired
//! with this domain, not yet paired, or paired only with other domains of
//! the same name (a lookalike: the app does not offer it).

use std::collections::HashMap;
use std::sync::Arc;

use super::pairings::Pairing;
use super::tree::EntryRef;
use crate::entry::opened::OpenedEntry;

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
    ascii_host(host.strip_suffix('.').unwrap_or(host))
}

/// `text` as a host name in ASCII: lower case, with every internationalised
/// label in its `xn--` form (UTS-46, as browsers map names). `None` if it is
/// not a host name.
pub fn ascii_host(text: &str) -> Option<String> {
    let host = if text.is_ascii() { text.to_ascii_lowercase() } else { idna::domain_to_ascii(text).ok()? };
    is_host(&host).then_some(host)
}

/// The host a part of an entry's path names, in ASCII ([`ascii_host`]):
/// a host name with a dot, or an IP address. `github.com` for `GitHub.com`;
/// `None` for a word such as `GitHub` or `alice`.
fn part_host(part: &str) -> Option<String> {
    ascii_host(part).filter(|host| host.contains('.'))
}

fn is_scheme(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Whether a browser would read `host` as an IPv4 address: its last label
/// is a number, in decimal or `0x` hex (the URL Standard's "ends in a
/// number"). That covers every form, canonical or not (`127.1`,
/// `0x7f.0.0.1`, `0177.0.0.1`, `2130706433`, `10.0.0.0x1`).
fn ends_in_number(host: &str) -> bool {
    let Some(last) = host.rsplit('.').next() else { return false };
    if !last.is_empty() && last.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    last.strip_prefix("0x")
        .or_else(|| last.strip_prefix("0X"))
        .is_some_and(|hex| hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The registrable domain of `host`, e.g. `example.co.uk` for
/// `www.example.co.uk`. `None` for a bare word and for an IP address in
/// any form, which only ever match exactly, as written.
///
/// Known limit: a multi-tenant host the Public Suffix List does not split
/// (`sharepoint.com`, `atlassian.net`, `zendesk.com`, `wordpress.com`,
/// `myshopify.com`) is one registrable domain, so one tenant's entry is
/// offered on every other tenant's site (docs/design.md 2.10).
pub(crate) fn registrable(host: &str) -> Option<&str> {
    if ends_in_number(host) {
        return None;
    }
    psl::domain_str(host)
}

/// The site `host` is on: its registrable domain, or the host itself where
/// there is none (an IP address).
fn site_of(host: &str) -> String {
    registrable(host).unwrap_or(host).to_owned()
}

pub const MIN_NAME_LEN: usize = 3;

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

/// The registrable domain a service identifier is on:
/// `bankofamerica.com` for `https://www.bankofamerica.com/login`. `None`
/// where there is none: a bare word, an IP address, a public suffix.
pub fn registrable_domain(service: String) -> Option<String> {
    let host = host_of(&service)?;
    registrable(&host).map(str::to_owned)
}

/// The host of a service identifier as AutoFill names the site, the way
/// [`display_host`] shows it: `github.com` for `https://GitHub.com/login`.
#[uniffi::export]
pub fn service_host(service: String) -> Option<String> {
    host_of(&service)
}

/// Whether `part` is a name iOS can be given as a suggestion's domain: an
/// ASCII DNS name with a dot, not an IP address.
fn is_plain_host(part: &str) -> bool {
    if part.len() > 253 || part.starts_with('.') || part.ends_with('.') {
        return false;
    }
    let labels: Vec<&str> = part.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        && labels.last().is_some_and(|last| last.chars().any(|c| c.is_ascii_alphabetic()))
}

/// The domain the suggestions above the keyboard give the entry at `path`:
/// the last part of its path that is a plain ASCII host name with a dot
/// (`web/github.com`, `github.com/alice`), in lower case, if that is a
/// registrable domain or a name under one. `None` for names like `GitHub`,
/// IP addresses, and public suffixes (`co.uk`, `github.io`), which iOS
/// might offer on every site under them.
#[uniffi::export]
pub fn suggestion_domain(path: String) -> Option<String> {
    let part = path.split('/').rev().find(|part| is_plain_host(part))?.to_ascii_lowercase();
    registrable(&part).is_some().then_some(part)
}

/// Folder names that sort entries by kind rather than name a site:
/// `bank/chase` is Chase, not bank.com. Squeezed like [`name_key`].
/// A folder with one of these names never matches by name.
pub const CATEGORY_FOLDERS: &[&str] = &[
    "accounts", "apps", "archive", "bank", "banking", "banks", "bills", "business", "cloud", "crypto", "dev",
    "education", "email", "emails", "entertainment", "family", "finance", "food", "forums", "games", "gaming",
    "government", "health", "home", "insurance", "internet", "logins", "mail", "media", "misc", "money", "music",
    "network", "news", "old", "online", "other", "personal", "phone", "private", "school", "servers", "services",
    "shop", "shopping", "sites", "social", "store", "stores", "streaming", "tax", "taxes", "tech", "travel",
    "utilities", "video", "web", "websites", "wifi", "work", "www",
];

/// Whether `path` matches the site named `name` by name ([`site_name`]).
///
/// Eligible parts, and only when no part before the entry's own name is
/// a host (`github.com/bankofamerica` is a user on github.com, as the app
/// derives user names):
/// - the entry's own name: `bankofamerica`, `bank/bankofamerica`;
/// - its direct parent folder, as in a `site/user` layout
///   (`bankofamerica/alice`), unless that folder's name is a category
///   ([`CATEGORY_FOLDERS`]): `bank/chase` does not match bank.com.
///
/// No other folder counts: `bankofamerica/old/alice` does not match.
pub fn names_site(path: &str, name: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let Some((last, folders)) = parts.split_last() else { return false };
    if folders.iter().any(|part| part_host(part).is_some()) {
        return false;
    }
    if name_key(last).as_deref() == Some(name) {
        return true;
    }
    folders
        .last()
        .and_then(|parent| name_key(parent))
        .is_some_and(|parent| parent == name && !CATEGORY_FOLDERS.contains(&parent.as_str()))
}

/// How well one part of an entry's path fits the host: 0 exact, 1 same
/// registrable domain.
fn fit(part: &str, host: &str, host_domain: Option<&str>) -> Option<u8> {
    if ascii_host(part).as_deref() == Some(host) {
        return Some(0);
    }
    let part = part_host(part)?;
    let domain = registrable(&part)?;
    (Some(domain) == host_domain).then_some(1)
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

/// Whether a match fills without asking: by host, or by a name already
/// filled on this domain.
#[uniffi::export]
pub fn is_trusted_match(kind: MatchKind) -> bool {
    rank(&kind) <= 2
}

/// Every entry that matches `service` by host or by name, best first:
/// exact host, same registrable domain, then name matches paired here, not
/// yet paired, and paired only elsewhere. Within each, the shorter path,
/// then alphabetically. `pairings` are the confirmed pairings
/// ([`super::pairings`]).
pub fn match_site(entries: &[EntryRef], service: &str, pairings: &[Pairing]) -> Vec<MatchedEntry> {
    let Some(host) = host_of(service) else {
        return Vec::new();
    };
    let host_domain = registrable(&host);
    let site = site_of(&host);
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

/// Whether a service may be matched automatically at all: printable ASCII
/// only, and a host with a dot. [`match_site`] matches a dotless host
/// (`http://github/`, an intranet name anyone can register) exactly
/// against an entry called `GitHub`; AutoFill does not offer that.
fn is_plain(service: &str) -> bool {
    !service.is_empty()
        && service.bytes().all(|b| (0x21..0x7F).contains(&b))
        && host_of(service).is_some_and(|host| host.contains('.'))
}

/// The entries AutoFill offers for `services` (the web addresses or hosts
/// iOS names), by [`match_site`] for each plain one ([`is_plain`]): each
/// entry once, as its best match, best first, and in the order found within
/// a kind.
pub fn match_services(entries: &[EntryRef], services: &[String], pairings: &[Pairing]) -> Vec<MatchedEntry> {
    let mut best: HashMap<String, (usize, MatchedEntry)> = HashMap::new();
    let mut order = 0;
    for service in services.iter().filter(|service| is_plain(service)) {
        for found in match_site(entries, service, pairings) {
            order += 1;
            if best.get(&found.entry.path).is_some_and(|(_, known)| rank(&known.kind) <= rank(&found.kind)) {
                continue;
            }
            best.insert(found.entry.path.clone(), (order, found));
        }
    }
    let mut found: Vec<(usize, MatchedEntry)> = best.into_values().collect();
    found.sort_by_key(|(order, found)| (rank(&found.kind), *order));
    found.into_iter().map(|(_, found)| found).collect()
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

/// How to show a host or domain to the person: lower case, no trailing
/// dot, and always the `xn--` form for a name with non-ASCII letters, so
/// `pаypal.com` with a Cyrillic `а` is shown as `xn--pypal-4ve.com`: many
/// accented Latin letters pass for plain ones at a glance (`ı`, `ł`, `ġ`…),
/// and telling which do is a job for a full confusables table. Text that is
/// not a host comes back as it was.
#[uniffi::export]
pub fn display_host(host: String) -> String {
    let trimmed = host.trim();
    ascii_host(trimmed.strip_suffix('.').unwrap_or(trimmed)).unwrap_or(host)
}

/// What an entry's own web address says about the site being filled
/// ([`check_entry_url`]).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum UrlCheck {
    /// No address field with a usable host, or no site to compare with.
    NoUrl,
    /// An address field is on the site's registrable domain.
    Same,
    /// Every address field is on another site. Both are registrable
    /// domains (the host where there is none), as [`display_host`] shows
    /// them: `url` the entry's first, `site` the site being filled.
    Differs { url: String, site: String },
}

/// The site an address field names: the registrable domain of its host,
/// or the host itself where there is none (an IP address). Tolerant of how
/// people write them: `northwind.example`, `HTTPS://Login.Northwind.Example:8443/x`,
/// `<https://northwind.example>`, a trailing dot, a note after a space.
/// `None` for a value without a host with a dot, and for a scheme other
/// than http and https (`androidapp://com.example`, `mailto:`).
pub fn url_site(value: &str) -> Option<String> {
    let value = value.trim().trim_start_matches('<');
    let token = value.split_whitespace().next()?.trim_end_matches('>');
    if let Some((scheme, _)) = token.split_once("://")
        && is_scheme(scheme)
        && !scheme.eq_ignore_ascii_case("http")
        && !scheme.eq_ignore_ascii_case("https")
    {
        return None;
    }
    if token.get(..7).is_some_and(|start| start.eq_ignore_ascii_case("mailto:")) {
        return None;
    }
    let host = host_of(token)?;
    if !host.contains('.') {
        return None;
    }
    Some(site_of(&host))
}

/// Compares address field values (`url: …` lines) with the sites being
/// filled ([`check_entry_url`]): [`UrlCheck::Same`] when any address is on
/// the registrable domain of any of `services`.
pub fn check_urls(values: &[&str], services: &[String]) -> UrlCheck {
    let urls: Vec<String> = values.iter().filter_map(|value| url_site(value)).collect();
    let sites: Vec<String> = services
        .iter()
        .filter_map(|service| host_of(service))
        .map(|host| site_of(&host))
        .collect();
    let (Some(url), Some(site)) = (urls.first(), sites.first()) else {
        return UrlCheck::NoUrl;
    };
    if urls.iter().any(|url| sites.contains(url)) {
        return UrlCheck::Same;
    }
    UrlCheck::Differs { url: display_host(url.clone()), site: display_host(site.clone()) }
}

/// Checks an opened entry's own address fields ([`EntryDoc::addresses`])
/// against the sites being filled, for AutoFill before it hands the
/// password over. Reads the fields only; keeps nothing.
///
/// [`EntryDoc::addresses`]: crate::entry::EntryDoc::addresses
#[uniffi::export]
pub fn check_entry_url(entry: Arc<OpenedEntry>, services: Vec<String>) -> UrlCheck {
    entry.read(|doc| check_urls(&doc.addresses(), &services)).unwrap_or(UrlCheck::NoUrl)
}

/// The user name AutoFill fills for the entry at `path`: the entry's own
/// ([`EntryDoc::filled_username`]), else the path's guess ([`path_user`]).
///
/// [`EntryDoc::filled_username`]: crate::entry::EntryDoc::filled_username
#[uniffi::export]
pub fn fill_username(entry: Arc<OpenedEntry>, path: String) -> Option<String> {
    entry.read(|doc| doc.filled_username().map(str::to_owned)).flatten().or_else(|| path_user(&path))
}

/// The user name a path gives in a `site/user` layout: the entry's name
/// when a folder above it is a host name with a dot (`github.com/alice`,
/// `servers/example.net/root`). `None` when the entry's own name is the
/// host (`web/github.com`) or no part is one (`bank/chase`), and an IP
/// address names no user (`home/192.168.1.1/admin`).
pub fn path_user(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    let (last, folders) = parts.split_last()?;
    let is_site = |part: &str| part_host(part).is_some_and(|host| !ends_in_number(&host));
    if is_site(last) || !folders.iter().any(|part| is_site(part)) || last.is_empty() {
        return None;
    }
    Some((*last).to_owned())
}
