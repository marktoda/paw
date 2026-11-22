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
//! `github.com`, `github.com.evil.example` is not `github.com`, and an entry
//! called just `github` matches no site at all: anyone can register
//! `github.example`, and offering a password there is how phishing works.
//!
//! Hosts are compared as lower-case text. An internationalised name matches
//! only if the entry spells it the same way the system reports it.

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
fn registrable(host: &str) -> Option<&str> {
    let last = host.rsplit('.').next()?;
    if last.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    psl::domain_str(host)
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
