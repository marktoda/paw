//! Search over names, and matching entries to a website for AutoFill. Both
//! are pure functions of the entry list, tested as tables.

use paw_core::store::search::{HighlightRange, search};
use paw_core::store::service::{host_of, match_service};
use paw_core::store::tree::EntryRef;

fn entries(paths: &[&str]) -> Vec<EntryRef> {
    paths
        .iter()
        .map(|path| {
            let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
            EntryRef { path: (*path).into(), name: name.into(), folder: folder.into() }
        })
        .collect()
}

const STORE: &[&str] = &[
    "Home Wi-Fi",
    "git",
    "web/github.com",
    "web/gitlab.com",
    "web/my-git-mirror",
    "web/digital-archive",
    "web/Zürich Tourism",
    "web/cafe\u{301}-menu",
    "git/hosting",
    "legit/notes",
    "email/fastmail",
    "email/work",
    "work/email",
    "work/vpn",
    "home/Guest Wi-Fi",
    "servers/example.net/root",
];

fn found(query: &str) -> Vec<String> {
    search(&entries(STORE), query, 100).into_iter().map(|hit| hit.entry.path).collect()
}

#[test]
fn results_are_ordered_by_how_well_the_name_fits() {
    let cases: &[(&str, &[&str])] = &[
        // Whole name, name prefix (shorter path first), a word in the name,
        // a folder, inside the name, inside a folder.
        (
            "git",
            &[
                "git",
                "web/github.com",
                "web/gitlab.com",
                "web/my-git-mirror",
                "git/hosting",
                "web/digital-archive",
                "legit/notes",
            ],
        ),
        ("email", &["work/email", "email/work", "email/fastmail"]),
        ("wi-fi", &["Home Wi-Fi", "home/Guest Wi-Fi"]),
        ("fi", &["Home Wi-Fi", "home/Guest Wi-Fi"]),
        ("home", &["Home Wi-Fi", "home/Guest Wi-Fi"]),
        ("root", &["servers/example.net/root"]),
        ("example", &["servers/example.net/root"]),
        ("net", &["servers/example.net/root"]),
        ("hub", &["web/github.com"]),
        ("work", &["email/work", "work/vpn", "work/email"]),
        ("nothing-like-this", &[]),
    ];
    for (query, expected) in cases {
        assert_eq!(found(query), *expected, "query {query:?}");
    }
}

#[test]
fn letter_case_and_accents_do_not_matter() {
    let cases: &[(&str, &[&str])] = &[
        ("GITHUB", &["web/github.com"]),
        ("zurich", &["web/Zürich Tourism"]),
        ("ZÜRICH", &["web/Zürich Tourism"]),
        ("zu\u{308}rich", &["web/Zürich Tourism"]),
        ("café", &["web/cafe\u{301}-menu"]),
        ("cafe", &["web/cafe\u{301}-menu"]),
        ("guest WI", &["home/Guest Wi-Fi"]),
    ];
    for (query, expected) in cases {
        assert_eq!(found(query), *expected, "query {query:?}");
    }
}

#[test]
fn every_word_of_the_query_must_match_somewhere_in_the_path() {
    let cases: &[(&str, &[&str])] = &[
        ("web git", &["web/github.com", "web/gitlab.com", "web/my-git-mirror", "web/digital-archive"]),
        ("git web", &["web/github.com", "web/gitlab.com", "web/my-git-mirror", "web/digital-archive"]),
        ("  hub   web ", &["web/github.com"]),
        ("email work", &["email/work", "work/email"]),
        ("web/git", &["web/github.com", "web/gitlab.com"]),
        ("git vpn", &[]),
        ("", &[]),
        ("   ", &[]),
    ];
    for (query, expected) in cases {
        assert_eq!(found(query), *expected, "query {query:?}");
    }
}

#[test]
fn the_limit_keeps_the_best_results() {
    let hits = search(&entries(STORE), "git", 2);
    let paths: Vec<_> = hits.iter().map(|hit| hit.entry.path.as_str()).collect();
    assert_eq!(paths, ["git", "web/github.com"]);
    assert!(search(&entries(STORE), "git", 0).is_empty());
}

fn range(start: u32, len: u32) -> HighlightRange {
    HighlightRange { start, len }
}

#[test]
fn hits_say_what_to_highlight_in_the_name_and_in_the_folder() {
    let store = entries(STORE);
    let one = |query: &str, path: &str| {
        search(&store, query, 100).into_iter().find(|hit| hit.entry.path == path).unwrap()
    };

    let hit = one("hub", "web/github.com");
    assert_eq!((hit.name_ranges, hit.folder_ranges), (vec![range(3, 3)], vec![]));

    let hit = one("web git", "web/github.com");
    assert_eq!((hit.name_ranges, hit.folder_ranges), (vec![range(0, 3)], vec![range(0, 3)]));

    // A word that spans the `/` is split between the two lines.
    let hit = one("web/git", "web/gitlab.com");
    assert_eq!((hit.name_ranges, hit.folder_ranges), (vec![range(0, 3)], vec![range(0, 3)]));

    // Overlapping words become one range.
    let hit = one("gith thub", "web/github.com");
    assert_eq!(hit.name_ranges, vec![range(0, 6)]);

    // The best place for a word is highlighted, not the first: the folder
    // `git` comes earlier in "git/hosting" than any name.
    let hit = one("git", "git/hosting");
    assert_eq!((hit.name_ranges, hit.folder_ranges), (vec![], vec![range(0, 3)]));

    let hit = one("net", "servers/example.net/root");
    assert_eq!(hit.folder_ranges, vec![range(16, 3)]);
}

#[test]
fn highlight_ranges_count_utf16_units_of_the_text_as_written() {
    let store = entries(&["web/cafe\u{301}-menu", "😀 smile/Zürich 🏔 peak"]);
    let one = |query: &str| search(&store, query, 10).remove(0);

    // "cafe" + combining accent is five units; the accent is highlighted
    // along with its letter.
    assert_eq!(one("cafe").name_ranges, vec![range(0, 5)]);
    assert_eq!(one("menu").name_ranges, vec![range(6, 4)]);

    // An emoji is two UTF-16 units.
    let hit = one("peak smile");
    assert_eq!(hit.name_ranges, vec![range(10, 4)]);
    assert_eq!(hit.folder_ranges, vec![range(3, 5)]);
    assert_eq!(one("zurich").name_ranges, vec![range(0, 6)]);
}

#[test]
fn a_service_identifier_is_reduced_to_its_host() {
    let cases = [
        ("github.com", Some("github.com")),
        ("Accounts.GitHub.com", Some("accounts.github.com")),
        ("https://github.com", Some("github.com")),
        ("https://github.com/login?return_to=https://evil.example", Some("github.com")),
        ("https://user:secret@github.com:443/login", Some("github.com")),
        ("github.com:8443", Some("github.com")),
        ("github.com.", Some("github.com")),
        ("  https://github.com/  ", Some("github.com")),
        ("http://192.168.1.1/admin", Some("192.168.1.1")),
        // What a browser would connect to, not what the text starts with.
        ("https://github.com@evil.example/", Some("evil.example")),
        ("https://evil.example\\@github.com/", Some("evil.example")),
        ("https://github.com.evil.example/github.com", Some("github.com.evil.example")),
        ("https://evil.example#github.com", Some("evil.example")),
        ("https://evil.example?github.com", Some("evil.example")),
        ("", None),
        ("https://", None),
        ("not a host", None),
        ("a..b", None),
        ("https://[::1]:8080/", None),
    ];
    for (service, expected) in cases {
        assert_eq!(host_of(service).as_deref(), expected, "{service:?}");
    }
}

const SITES: &[&str] = &[
    "web/github.com",
    "web/gist.github.com",
    "github.com/work-account",
    "web/github.com/personal",
    "web/hub.com",
    "web/github.com.evil.example",
    "web/notgithub.com",
    "web/github.community",
    "web/github",
    "web/github.io",
    "web/alice.github.io",
    "web/bob.github.io",
    "bank/example.co.uk",
    "bank/other.co.uk",
    "home/192.168.1.1",
    "home/10.0.1.1",
    "home/nas.lan",
    "email/fastmail",
    "Home Wi-Fi",
];

fn offered(service: &str) -> Vec<(String, bool)> {
    match_service(&entries(SITES), service)
        .into_iter()
        .map(|m| (m.entry.path, m.exact))
        .collect()
}

#[test]
fn entries_are_offered_for_their_host_then_for_its_registrable_domain() {
    let cases: &[(&str, &[(&str, bool)])] = &[
        (
            "github.com",
            &[
                ("web/github.com", true),
                ("github.com/work-account", true),
                ("web/github.com/personal", true),
                ("web/gist.github.com", false),
            ],
        ),
        (
            "https://accounts.github.com/login",
            &[
                ("web/github.com", false),
                ("web/gist.github.com", false),
                ("github.com/work-account", false),
                ("web/github.com/personal", false),
            ],
        ),
        (
            "gist.github.com",
            &[
                ("web/gist.github.com", true),
                ("web/github.com", false),
                ("github.com/work-account", false),
                ("web/github.com/personal", false),
            ],
        ),
        ("GitHub.COM.", &[
            ("web/github.com", true),
            ("github.com/work-account", true),
            ("web/github.com/personal", true),
            ("web/gist.github.com", false),
        ]),
        ("www.example.co.uk", &[("bank/example.co.uk", false)]),
        ("http://192.168.1.1/", &[("home/192.168.1.1", true)]),
        ("files.nas.lan", &[("home/nas.lan", false)]),
    ];
    for (service, expected) in cases {
        let expected: Vec<(String, bool)> =
            expected.iter().map(|(path, exact)| ((*path).to_owned(), *exact)).collect();
        assert_eq!(offered(service), expected, "service {service:?}");
    }
}

#[test]
fn a_piece_of_a_name_never_matches() {
    let cases: &[(&str, &[&str])] = &[
        // `hub.com` is not `github.com`, in either direction.
        ("hub.com", &["web/hub.com"]),
        // The attacker's host ends in a real one, or starts with it.
        ("github.com.evil.example", &["web/github.com.evil.example"]),
        ("evil.example", &["web/github.com.evil.example"]),
        ("notgithub.com", &["web/notgithub.com"]),
        ("github.community", &["web/github.community"]),
        // An entry called just `github` is a word, not a host: anyone can
        // register `github.example`.
        ("github.example", &[]),
        ("github.xyz", &[]),
        ("fastmail.com", &[]),
        // Each `github.io` site belongs to someone else.
        ("alice.github.io", &["web/alice.github.io"]),
        ("mallory.github.io", &[]),
        ("github.io", &["web/github.io"]),
        // A public suffix is nobody's domain.
        ("co.uk", &[]),
        ("other.co.uk", &["bank/other.co.uk"]),
        // Addresses match exactly or not at all.
        ("10.0.1.1", &["home/10.0.1.1"]),
        ("172.16.1.1", &[]),
        ("com", &[]),
        ("", &[]),
        ("https://github.com@evil.example/", &["web/github.com.evil.example"]),
    ];
    for (service, expected) in cases {
        let paths: Vec<String> = offered(service).into_iter().map(|(path, _)| path).collect();
        assert_eq!(paths, *expected, "service {service:?}");
    }
}
