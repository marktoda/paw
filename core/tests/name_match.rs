//! Matching entries named without a domain (`bankofamerica`) to sites, and
//! the pairings that say which domains such an entry has been filled on.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use paw_core::store::PassStore;
use paw_core::store::pairings::{FILE, MAX_ENTRIES, MAX_SITES, PairedSite, Pairing, PairingFile, SitePairings};
use paw_core::store::service::{MatchKind, NamePairing, match_service, match_site, name_key, site_name};
use paw_core::store::tree::EntryRef;

fn entries(paths: &[&str]) -> Vec<EntryRef> {
    let mut out: Vec<EntryRef> = paths
        .iter()
        .map(|path| {
            let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
            EntryRef { path: (*path).into(), name: name.into(), folder: folder.into() }
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Entries named the many ways people name them.
const STORE: &[&str] = &[
    "bankofamerica",
    "bank/bankofamerica",
    "Finance/Bank of America",
    "finance/bank-of-america",
    "finance/BANK_OF_AMERICA",
    "alaskaair",
    "travel/alaska",
    "bank/chase",
    "barclays",
    "github",
    "web/github.com",
    "web/GitHub",
    "github.com/bankofamerica",
    "contoso/alice",
    "contoso/bob",
    "web/github.com/alaskaair",
    "ab",
    "evil",
    "paypal",
    "Zürich",
    "xn--bcher-kva",
    "web/fastmail",
    "co",
    "localhost",
    "bankofamerica/alice",
    "bankofamerica/old/alice",
    "mail/fastmail",
    "Shop/amazon",
    "work/vpn",
    "home/router",
    "social/mastodon",
];

/// The name-matched paths for `service`, as `(path, pairing)` with no
/// pairings known: every one unpaired.
fn by_name(service: &str) -> Vec<String> {
    match_site(&entries(STORE), service, &[])
        .into_iter()
        .filter(|m| matches!(m.kind, MatchKind::Name { .. }))
        .map(|m| m.entry.path)
        .collect()
}

#[test]
fn a_part_matches_only_the_first_label_of_the_registrable_domain() {
    let boa: &[&str] = &[
        "bankofamerica",
        "bank/bankofamerica",
        "finance/bank-of-america",
        "Finance/Bank of America",
        "finance/BANK_OF_AMERICA",
        // A `site/user` layout matches by its parent folder.
        "bankofamerica/alice",
    ];
    let cases: &[(&str, &[&str])] = &[
        // The plain cases.
        ("https://www.bankofamerica.com/login", boa),
        ("bankofamerica.com", boa),
        ("https://secure.bankofamerica.com", boa),
        ("https://alaskaair.com", &["alaskaair"]),
        ("https://www.alaskaair.com/", &["alaskaair"]),
        // Letter case, a trailing dot, a port, user info.
        ("https://WWW.BankOfAmerica.COM./", boa),
        ("https://www.bankofamerica.com:443/x", boa),
        ("https://someone@www.bankofamerica.com/", boa),
        // Separators in the site's label count as nothing, as in the entry.
        ("https://bank-of-america.com", boa),
        // Multi-level public suffixes.
        ("https://www.barclays.co.uk", &["barclays"]),
        ("https://barclays.co.uk", &["barclays"]),
        // A subdomain of the named site; and the host-named entry wins over
        // the name for github.com.
        ("https://gist.github.com", &["github", "web/GitHub"]),
        ("https://github.com", &["github", "web/GitHub"]),
        // A lookalike with the same name is still a name match: the
        // pairings are what tell them apart.
        ("https://bankofamerica.co", boa),
        ("https://bankofamerica.cm", boa),
        ("https://www.bankofamerica.net", boa),
        ("https://github.example", &["github", "web/GitHub"]),
        // The registrable domain's label, not a label further left.
        ("https://bankofamerica.evil.com", &["evil"]),
        ("https://paypal.com.evil.example", &["evil"]),
        ("https://www.bankofamerica.com.evil.example/", &["evil"]),
        ("https://github.com@evil.example/", &["evil"]),
        ("https://evil.example/?next=https://www.bankofamerica.com", &["evil"]),
        // `github.io` is a public suffix: each site under it is someone's
        // own, named by its own label. `alice.github.io` is not `github`;
        // it is `alice`, so the entry named `alice` (a user, to a person,
        // but nothing in the name says so) matches, behind the first-use
        // confirm.
        ("https://alice.github.io", &["contoso/alice", "bankofamerica/alice", "bankofamerica/old/alice"]),
        ("https://mallory.github.io", &[]),
        ("https://github.io", &[]),
        ("https://contoso.github.io", &["contoso/alice", "contoso/bob"]),
        // A public suffix alone has no name.
        ("https://co.uk", &[]),
        ("co", &[]),
        // Whole parts only: never a prefix, a suffix or a piece.
        // A category folder is not a site: `bank/chase` is Chase.
        ("https://bank.com", &[]),
        ("https://web.com", &[]),
        ("https://www.mail.com", &[]),
        ("https://shop.com", &[]),
        ("https://work.com", &[]),
        ("https://home.com", &[]),
        ("https://social.com", &[]),
        ("https://amazon.com", &["Shop/amazon"]),
        ("https://chase.com", &["bank/chase"]),
        ("https://alaska.com", &["travel/alaska"]),
        ("https://america.com", &[]),
        ("https://bankofamerica2.com", &[]),
        ("https://mybankofamerica.com", &[]),
        ("https://alaskaairlines.com", &[]),
        // Too short to be a name.
        ("https://ab.com", &[]),
        // Addresses and dotless hosts never match by name.
        ("http://192.168.1.1/", &[]),
        ("http://localhost:8080/", &[]),
        ("http://bankofamerica/", &[]),
        ("bankofamerica", &[]),
        ("https://[::1]/", &[]),
        ("", &[]),
        // Internationalised names: a `xn--` label never matches; a plain
        // ASCII label under an internationalised suffix does.
        ("https://xn--bcher-kva.com", &[]),
        ("https://xn--bnkofamerica-x9a.com", &[]),
        ("https://bankofamerica.xn--p1ai", boa),
        ("https://zürich.com", &[]),
        // A user in a `site/user` layout is not a site: `github.com/bankofamerica`.
        // A folder named like the site is: `contoso/alice`.
        ("https://contoso.com", &["contoso/alice", "contoso/bob"]),
        ("https://fastmail.com", &["web/fastmail", "mail/fastmail"]),
        ("https://www.fastmail.com", &["web/fastmail", "mail/fastmail"]),
        // Only the direct parent counts: `bankofamerica/old/alice` is not
        // matched by `bankofamerica` (checked under bankofamerica.com above)
        // and `old` is a category.
        ("https://old.com", &[]),
    ];
    assert!(cases.len() >= 40);
    for (service, expected) in cases {
        let mut got = by_name(service);
        let mut want: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
        got.sort();
        want.sort();
        assert_eq!(got, want, "service {service:?}");
    }
}

#[test]
fn the_last_part_is_a_user_when_a_host_comes_before_it() {
    // `github.com/bankofamerica` is someone's GitHub account.
    let found = by_name("https://bankofamerica.com");
    assert!(!found.contains(&"github.com/bankofamerica".to_owned()));
    // `web/github.com/alaskaair` likewise.
    assert!(!by_name("https://alaskaair.com").contains(&"web/github.com/alaskaair".to_owned()));
    // On github.com the same entries match by host, never twice.
    let on_github = match_site(&entries(STORE), "https://github.com", &[]);
    let paths: Vec<&str> = on_github.iter().map(|m| m.entry.path.as_str()).collect();
    assert_eq!(&paths[..3], &["web/github.com", "github.com/bankofamerica", "web/github.com/alaskaair"]);
    assert!(on_github[..3].iter().all(|m| m.kind == MatchKind::ExactHost));
}

#[test]
fn names_and_site_names_are_squeezed_the_same_way() {
    let names = [
        ("bankofamerica", Some("bankofamerica")),
        ("Bank of America", Some("bankofamerica")),
        ("bank-of-america", Some("bankofamerica")),
        ("BANK_OF_AMERICA", Some("bankofamerica")),
        ("github.com", None),
        ("Zürich", None),
        ("ab", None),
        ("a-b", None),
        ("bank!", None),
        ("", None),
    ];
    for (part, want) in names {
        assert_eq!(name_key(part).as_deref(), want, "{part:?}");
    }
    let sites = [
        ("www.bankofamerica.com", Some("bankofamerica")),
        ("bank-of-america.com", Some("bankofamerica")),
        ("alice.github.io", Some("alice")),
        ("github.io", None),
        ("co.uk", None),
        ("192.168.1.1", None),
        ("localhost", None),
        ("xn--bcher-kva.com", None),
        ("ab.com", None),
    ];
    for (host, want) in sites {
        assert_eq!(site_name(host).as_deref(), want, "{host:?}");
    }
}

#[test]
fn host_matches_are_unchanged_and_come_first() {
    let all = entries(STORE);
    let tier_one: Vec<String> = match_service(&all, "https://github.com").into_iter().map(|m| m.entry.path).collect();
    let site = match_site(&all, "https://github.com", &[]);
    let hosts: Vec<String> = site
        .iter()
        .filter(|m| matches!(m.kind, MatchKind::ExactHost | MatchKind::SameDomain))
        .map(|m| m.entry.path.clone())
        .collect();
    assert_eq!(hosts, tier_one);
    assert_eq!(site.iter().map(|m| m.site.as_str()).collect::<HashSet<_>>(), HashSet::from(["github.com"]));
    // Name matches follow every host match.
    let first_name = site.iter().position(|m| matches!(m.kind, MatchKind::Name { .. })).unwrap();
    assert_eq!(first_name, tier_one.len());
}

fn pairing(entry: &str, sites: &[&str]) -> Pairing {
    Pairing { entry: entry.into(), sites: sites.iter().map(|s| (*s).into()).collect() }
}

fn kinds(service: &str, pairings: &[Pairing]) -> Vec<(String, MatchKind)> {
    match_site(&entries(STORE), service, pairings).into_iter().map(|m| (m.entry.path, m.kind)).collect()
}

fn kind_of(service: &str, pairings: &[Pairing], path: &str) -> MatchKind {
    kinds(service, pairings).into_iter().find(|(p, _)| p == path).map(|(_, k)| k).unwrap()
}

#[test]
fn a_name_match_is_paired_unpaired_or_paired_elsewhere() {
    let known = [pairing("bankofamerica", &["bankofamerica.com"]), pairing("contoso/alice", &["alaskaair.com"])];
    let paired = MatchKind::Name { pairing: NamePairing::Paired };
    let unpaired = MatchKind::Name { pairing: NamePairing::Unpaired };
    let elsewhere =
        MatchKind::Name { pairing: NamePairing::PairedElsewhere { sites: vec!["bankofamerica.com".into()] } };

    // Paired on the registrable domain: any host of it.
    assert_eq!(kind_of("https://www.bankofamerica.com", &known, "bankofamerica"), paired);
    assert_eq!(kind_of("https://secure.bankofamerica.com", &known, "bankofamerica"), paired);
    // Another entry of the same name is still unpaired.
    assert_eq!(kind_of("https://www.bankofamerica.com", &known, "bank/bankofamerica"), unpaired);
    // The same name on another domain: a lookalike.
    assert_eq!(kind_of("https://bankofamerica.co", &known, "bankofamerica"), elsewhere);
    assert_eq!(kind_of("https://bank-of-america.com", &known, "bankofamerica"), elsewhere);
    // A pairing for a domain of another name says nothing here.
    assert_eq!(kind_of("https://contoso.com", &known, "contoso/alice"), unpaired);
    // Order: paired, then unpaired, then paired elsewhere.
    let order = kinds("https://www.bankofamerica.com", &[pairing("finance/bank-of-america", &["bankofamerica.com"]),
        pairing("bank/bankofamerica", &["bankofamerica.co"])]);
    assert_eq!(order.first().unwrap().0, "finance/bank-of-america");
    assert_eq!(order.last().unwrap().0, "bank/bankofamerica");
    assert!(matches!(order.last().unwrap().1, MatchKind::Name { pairing: NamePairing::PairedElsewhere { .. } }));
}

// ---- The pairings file ----

/// A state directory with a store checked out in `repo`.
fn state_with(paths: &[&str]) -> (tempfile::TempDir, Arc<PassStore>, Arc<SitePairings>) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    for path in paths {
        let file = repo.join(format!("{path}.gpg"));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "not encrypted").unwrap();
    }
    std::fs::write(repo.join(".gpg-id"), "0000\n").unwrap();
    let store = PassStore::open(repo.to_str().unwrap().into()).unwrap();
    let pairings = SitePairings::open(dir.path().to_str().unwrap().into());
    (dir, store, pairings)
}

fn temps(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with(".site-pairings"))
        .collect()
}

#[test]
fn only_a_name_match_for_the_site_is_ever_paired() {
    let (dir, store, pairings) = state_with(&["bank/bankofamerica", "email/fastmail", "web/github.com", "github"]);
    let record = |entry: &str, service: &str| pairings.record_pairing(store.clone(), entry.into(), service.into()).unwrap();
    assert!(record("bank/bankofamerica", "https://www.bankofamerica.com/login"));
    // A search pick for another site is never paired.
    assert!(!record("email/fastmail", "https://www.bankofamerica.com"));
    // A host match needs no pairing.
    assert!(!record("web/github.com", "https://github.com"));
    // No entry, no name, an address, a dotless host, a public suffix.
    assert!(!record("bank/gone", "https://gone.com"));
    assert!(!record("github", "http://192.168.1.1"));
    assert!(!record("github", "http://github/"));
    assert!(!record("github", "https://github.io"));
    assert!(!record("github", "https://alice.github.io"));
    assert_eq!(pairings.pairings(), vec![pairing("bank/bankofamerica", &["bankofamerica.com"])]);
    assert!(dir.path().join(FILE).is_file());
    assert!(temps(dir.path()).is_empty());

    // The match now says paired, and a lookalike says where it was used.
    let found = store.match_site("https://bankofamerica.com".into(), Some(pairings.clone()));
    assert_eq!(found[0].kind, MatchKind::Name { pairing: NamePairing::Paired });
    assert_eq!(found[0].site, "bankofamerica.com");
    let lookalike = store.match_site("https://www.bankofamerica.co".into(), Some(pairings.clone()));
    assert_eq!(
        lookalike[0].kind,
        MatchKind::Name { pairing: NamePairing::PairedElsewhere { sites: vec!["bankofamerica.com".into()] } }
    );
    assert_eq!(lookalike[0].site, "bankofamerica.co");
    // Without pairings every name match is unpaired.
    let blind = store.match_site("https://bankofamerica.com".into(), None);
    assert_eq!(blind[0].kind, MatchKind::Name { pairing: NamePairing::Unpaired });
}

#[test]
fn recording_again_moves_a_site_to_the_end_and_keeps_few_sites() {
    let (_dir, store, pairings) = state_with(&["github"]);
    let sites = ["github.com", "github.co", "github.net", "github.org", "github.dev", "github.com"];
    for site in sites {
        assert!(pairings.record_pairing(store.clone(), "github".into(), format!("https://{site}")).unwrap());
    }
    let kept = pairings.pairings();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].sites.len(), MAX_SITES);
    assert_eq!(kept[0].sites, vec!["github.net", "github.org", "github.dev", "github.com"]);
}

#[test]
fn the_file_keeps_at_most_the_most_recent_entries() {
    let dir = tempfile::tempdir().unwrap();
    let file = PairingFile::new(dir.path());
    for i in 0..MAX_ENTRIES + 25 {
        file.record(&format!("site{i:04}"), &format!("site{i:04}.com"), None).unwrap();
    }
    let kept = file.load();
    assert_eq!(kept.len(), MAX_ENTRIES);
    assert_eq!(kept[0].entry, "site0025");
    assert_eq!(kept.last().unwrap().entry, format!("site{:04}", MAX_ENTRIES + 24));
    assert!(std::fs::metadata(file.path()).unwrap().len() < 64 * 1024);
}

#[test]
fn entries_no_longer_in_the_store_are_dropped_on_the_next_record() {
    let (dir, store, pairings) = state_with(&["github", "alaskaair"]);
    PairingFile::new(dir.path()).record("deleted/elsewhere", "deleted.com", None).unwrap();
    pairings.record_pairing(store, "alaskaair".into(), "https://alaskaair.com".into()).unwrap();
    assert_eq!(pairings.pairings(), vec![pairing("alaskaair", &["alaskaair.com"])]);
}

#[test]
fn a_corrupt_file_is_ignored_and_rewritten() {
    let (dir, store, pairings) = state_with(&["alaskaair"]);
    let path = dir.path().join(FILE);
    for junk in [
        &b"not json"[..],
        b"{\"version\":2,\"pairings\":[{\"entry\":\"alaskaair\",\"sites\":[\"alaskaair.com\"]}]}",
        b"{\"version\":1}",
        b"\xff\xfe\x00",
        b"",
    ] {
        std::fs::write(&path, junk).unwrap();
        assert!(pairings.pairings().is_empty(), "{junk:?}");
    }
    // Malformed items are left out; the good ones stay.
    std::fs::write(
        &path,
        br#"{"version":1,"pairings":[
            {"entry":"alaskaair","sites":["alaskaair.com","Alaskaair.com","co.uk","192.168.1.1","https://x.com",""]},
            {"entry":"","sites":["x.com"]},
            {"entry":"alaskaair","sites":["duplicate.com"]}]}"#,
    )
    .unwrap();
    assert_eq!(pairings.pairings(), vec![pairing("alaskaair", &["alaskaair.com"])]);
    // A huge file is not read.
    std::fs::write(&path, vec![b' '; 2 * 1024 * 1024]).unwrap();
    assert!(pairings.pairings().is_empty());
    // The next record writes a good file.
    std::fs::write(&path, b"garbage").unwrap();
    assert!(pairings.record_pairing(store, "alaskaair".into(), "https://www.alaskaair.com".into()).unwrap());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("{\"version\":1"), "{text}");
    assert_eq!(pairings.pairings(), vec![pairing("alaskaair", &["alaskaair.com"])]);
}

#[test]
fn writes_are_atomic_and_threads_lose_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    // Two "processes": separate objects on the same file, plus a reader.
    let writers: Vec<_> = (0..2)
        .map(|w| {
            let path = path.clone();
            std::thread::spawn(move || {
                let file = PairingFile::new(&path);
                for i in 0..40 {
                    file.record(&format!("w{w}/entry{i}"), &format!("entry{i}.com"), None).unwrap();
                }
            })
        })
        .collect();
    let reader = {
        let path = path.clone();
        std::thread::spawn(move || {
            for _ in 0..400 {
                let bytes = std::fs::read(path.join(FILE)).unwrap_or_default();
                // Never a half-written file: empty (not there yet) or whole JSON.
                assert!(bytes.is_empty() || serde_json::from_slice::<serde_json::Value>(&bytes).is_ok());
            }
        })
    };
    for writer in writers {
        writer.join().unwrap();
    }
    reader.join().unwrap();
    let kept = PairingFile::new(&path).load();
    assert_eq!(kept.len(), 80);
    // No temporary file is left behind.
    assert!(temps(&path).is_empty());
    let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(path.join(FILE)).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_stale_temporary_file_is_cleaned_and_a_fresh_one_is_left() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join(".site-pairings.tmp-fresh");
    std::fs::write(&fresh, "half").unwrap();
    let stale = dir.path().join(".site-pairings.tmp-stale");
    std::fs::write(&stale, "half").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
    std::fs::File::options().write(true).open(&stale).unwrap().set_modified(old).unwrap();
    PairingFile::new(dir.path()).record("alaskaair", "alaskaair.com", None).unwrap();
    assert!(fresh.exists());
    assert!(!stale.exists());
}

#[test]
fn clearing_forgets_one_entry_or_everything() {
    let (dir, store, pairings) = state_with(&["alaskaair", "github"]);
    pairings.record_pairing(store.clone(), "alaskaair".into(), "https://alaskaair.com".into()).unwrap();
    pairings.record_pairing(store.clone(), "github".into(), "https://github.com".into()).unwrap();
    pairings.clear_pairings(Some("github".into())).unwrap();
    assert_eq!(pairings.pairings(), vec![pairing("alaskaair", &["alaskaair.com"])]);
    pairings.clear_pairings(None).unwrap();
    assert!(pairings.pairings().is_empty());
    assert!(!dir.path().join(FILE).exists());
    // Clearing nothing is fine.
    pairings.clear_pairings(None).unwrap();
}

#[test]
fn nothing_is_recorded_once_the_store_is_removed() {
    let (dir, store, pairings) = state_with(&["alaskaair"]);
    let state = dir.path().to_path_buf();
    pairings.record_pairing(store.clone(), "alaskaair".into(), "https://alaskaair.com".into()).unwrap();
    // Removing the store deletes the state directory, pairings with it.
    std::fs::remove_dir_all(&state).unwrap();
    assert!(pairings.pairings().is_empty());
    // A late fill in the extension does not bring the folder back.
    assert!(pairings.record_pairing(store, "alaskaair".into(), "https://alaskaair.com".into()).is_err());
    assert!(!state.exists());
}

#[test]
fn moving_an_entry_keeps_its_pairings_and_forgetting_drops_them() {
    let dir = tempfile::tempdir().unwrap();
    let file = PairingFile::new(dir.path());
    file.record("bank/bankofamerica", "bankofamerica.com", None).unwrap();
    file.record("alaskaair", "alaskaair.com", None).unwrap();
    file.moved("bank/bankofamerica", "finance/bankofamerica").unwrap();
    assert_eq!(
        file.load(),
        vec![pairing("finance/bankofamerica", &["bankofamerica.com"]), pairing("alaskaair", &["alaskaair.com"])]
    );
    // Moving something with no pairings changes nothing.
    file.moved("nothing", "else").unwrap();
    file.clear(Some("alaskaair")).unwrap();
    assert_eq!(file.load(), vec![pairing("finance/bankofamerica", &["bankofamerica.com"])]);
}

#[test]
fn paired_sites_are_only_those_that_still_match_by_name() {
    // `alaskaair` and `github` match sites by name but were never filled
    // there: no suggestion is ever published for them.
    let (dir, store, pairings) =
        state_with(&["bankofamerica", "travel/alaska", "contoso/alice", "alaskaair", "github"]);
    let file = PairingFile::new(dir.path());
    file.record("bankofamerica", "bankofamerica.com", None).unwrap();
    file.record("bankofamerica", "bankofamerica.co", None).unwrap();
    // Renamed since: no longer matches its pairing.
    file.record("travel/alaska", "alaskaair.com", None).unwrap();
    // Deleted since.
    file.record("gone", "gone.com", None).unwrap();
    file.record("contoso/alice", "contoso.com", None).unwrap();
    assert_eq!(
        pairings.paired_sites(store),
        vec![
            PairedSite { entry: "bankofamerica".into(), site: "bankofamerica.co".into() },
            PairedSite { entry: "bankofamerica".into(), site: "bankofamerica.com".into() },
            PairedSite { entry: "contoso/alice".into(), site: "contoso.com".into() },
        ]
    );
}
