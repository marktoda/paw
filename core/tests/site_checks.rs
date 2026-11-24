//! What AutoFill checks besides an entry's name: international domain
//! names, the entry's own `url:` lines, and the user name it fills.

use paw_core::ffi::OpenedEntry;
use paw_core::store::service::{
    MatchKind, UrlCheck, check_entry_url, display_host, fill_username, host_of, match_service, match_site,
    path_user, url_site,
};
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

// MARK: International domain names

const IDN_STORE: &[&str] = &[
    "web/bücher.example",
    "web/xn--bcher-kva.example",
    "web/BÜCHER.example/alice",
    "web/bucher.example",
    "web/paypal.com",
    "web/p\u{0430}ypal.com", // Cyrillic а
    "web/shop.bu\u{0308}cher.example", // ü as u and a combining diaeresis
    "web/münchen.de",
    "web/xn--80ak6aa92e.xn--p1ai",
    "web/例え.jp",
    "web/Zürich Tourism",
];

fn hosts(service: &str) -> Vec<String> {
    match_service(&entries(IDN_STORE), service).into_iter().map(|m| m.entry.path).collect()
}

#[test]
fn international_names_match_in_their_ascii_form_on_both_sides() {
    let bucher = [
        "web/bücher.example",
        "web/BÜCHER.example/alice",
        "web/xn--bcher-kva.example",
        "web/shop.bu\u{0308}cher.example",
    ];
    let cases: &[(&str, &[&str])] = &[
        ("https://xn--bcher-kva.example/login", &bucher),
        ("https://XN--BCHER-KVA.EXAMPLE./", &bucher),
        ("https://bücher.example", &bucher),
        ("https://shop.xn--bcher-kva.example", &[
            "web/shop.bu\u{0308}cher.example",
            "web/bücher.example",
            "web/BÜCHER.example/alice",
            "web/xn--bcher-kva.example",
        ]),
        ("https://bucher.example", &["web/bucher.example"]),
        // Homographs: the Cyrillic а is another name, both ways.
        ("https://paypal.com", &["web/paypal.com"]),
        ("https://xn--pypal-4ve.com", &["web/p\u{0430}ypal.com"]),
        ("https://p\u{0430}ypal.com", &["web/p\u{0430}ypal.com"]),
        ("https://xn--mnchen-3ya.de", &["web/münchen.de"]),
        ("https://www.xn--80ak6aa92e.xn--p1ai", &["web/xn--80ak6aa92e.xn--p1ai"]),
        ("https://xn--r8jz45g.jp", &["web/例え.jp"]),
        ("https://xn--zrich-kva.example", &[]),
        ("https://xn--bcher-kva.example.attacker.example", &[]),
    ];
    for (service, want) in cases {
        let got = hosts(service);
        assert_eq!(got, *want, "{service}");
    }
}

#[test]
fn a_homograph_never_matches_by_name_either() {
    let store = entries(&["paypal", "bücher", "xn--bcher-kva"]);
    for service in ["https://xn--pypal-4ve.com", "https://xn--bcher-kva.example"] {
        let names: Vec<_> =
            match_site(&store, service, &[]).into_iter().filter(|m| matches!(m.kind, MatchKind::Name { .. })).collect();
        assert!(names.is_empty(), "{service}: {names:?}");
    }
}

#[test]
fn hosts_are_read_in_ascii() {
    let cases: &[(&str, Option<&str>)] = &[
        ("https://Bücher.Example./x", Some("xn--bcher-kva.example")),
        ("bücher。example", Some("xn--bcher-kva.example")),
        ("https://xn--bcher-kva.example", Some("xn--bcher-kva.example")),
        ("https://GitHub.com", Some("github.com")),
        ("https://exa mple.com", None),
    ];
    for (service, want) in cases {
        assert_eq!(host_of(service).as_deref(), *want, "{service}");
    }
}

#[test]
fn display_shows_unicode_only_for_latin_names() {
    let cases: &[(&str, &str)] = &[
        ("xn--bcher-kva.example", "bücher.example"),
        ("www.xn--bcher-kva.example", "www.bücher.example"),
        ("bücher.example", "bücher.example"),
        ("BÜCHER.example.", "bücher.example"),
        ("xn--mnchen-3ya.de", "münchen.de"),
        ("xn--strae-oqa.de", "straße.de"),
        ("xn--dbo-iwa2zka.pl", "żdźbło.pl"),
        ("github.com", "github.com"),
        ("GitHub.COM", "github.com"),
        // Homographs and other scripts stay in ASCII.
        ("p\u{0430}ypal.com", "xn--pypal-4ve.com"),
        ("xn--pypal-4ve.com", "xn--pypal-4ve.com"),
        ("xn--80ak6aa92e.xn--p1ai", "xn--80ak6aa92e.xn--p1ai"),
        ("例え.jp", "xn--r8jz45g.jp"),
        ("ex\u{03B1}mple.com", "xn--exmple-qxe.com"),
        // Latin letters that pass for plain ones.
        ("g\u{0131}thub.com", "xn--gthub-n4a.com"),
        ("\u{0138}raken.com", "xn--raken-t5a.com"),
        // Mixed Latin and Cyrillic in one name.
        ("bü\u{0441}her.example", "xn--bher-0ra631c.example"),
        // Not a host: as given.
        ("not a host", "not a host"),
    ];
    for (host, want) in cases {
        assert_eq!(display_host((*host).into()), *want, "{host}");
    }
}

// MARK: The entry's own url

#[test]
fn url_values_are_read_tolerantly() {
    let cases: &[(&str, Option<&str>)] = &[
        ("northwind.example", Some("northwind.example")),
        ("https://northwind.example", Some("northwind.example")),
        ("HTTPS://Login.NorthWind.Example:8443/sign-in?x=1", Some("northwind.example")),
        ("http://www.northwind.example./", Some("northwind.example")),
        ("northwind.example/login", Some("northwind.example")),
        ("<https://northwind.example>", Some("northwind.example")),
        ("https://northwind.example (the old one)", Some("northwind.example")),
        ("  www.northwind.co.uk  ", Some("northwind.co.uk")),
        ("https://user@northwind.example", Some("northwind.example")),
        ("https://bücher.example", Some("xn--bcher-kva.example")),
        ("http://192.168.1.1:8080", Some("192.168.1.1")),
        ("androidapp://com.northwind.app", None),
        ("mailto:you@northwind.example", None),
        ("none", None),
        ("n/a", None),
        ("", None),
    ];
    for (value, want) in cases {
        assert_eq!(url_site(value).as_deref(), *want, "{value:?}");
    }
}

fn opened(text: &str) -> std::sync::Arc<OpenedEntry> {
    OpenedEntry::from_plaintext(text.as_bytes().to_vec()).unwrap()
}

fn differs(url: &str, site: &str) -> UrlCheck {
    UrlCheck::Differs { url: url.into(), site: site.into() }
}

#[test]
fn the_entrys_url_is_checked_against_the_site() {
    let on = |site: &str| vec![site.to_owned()];
    let cases: Vec<(&str, Vec<String>, UrlCheck)> = vec![
        ("pw\nurl: https://northwind.example\n", on("https://www.northwind.example/login"), UrlCheck::Same),
        ("pw\nURL: northwind.example\n", on("https://login.northwind.example"), UrlCheck::Same),
        ("pw\nwebsite: northwind.example\n", on("https://northwind.example"), UrlCheck::Same),
        ("pw\nSite: northwind.example\n", on("https://northwind.example"), UrlCheck::Same),
        ("pw\nLogin URL: https://northwind.example/x\n", on("https://northwind.example"), UrlCheck::Same),
        (
            "pw\nurl: https://northwind.example\n",
            on("https://northwind.test"),
            differs("northwind.example", "northwind.test"),
        ),
        (
            "pw\nlogin url: https://northwind.example\n",
            on("https://login.northwind.test/"),
            differs("northwind.example", "northwind.test"),
        ),
        // Several lines: any one on the site will do.
        ("pw\nurl: https://sso.contoso.example\nurl: https://northwind.example\n", on("https://northwind.example"), UrlCheck::Same),
        ("pw\nurl: https://northwind.example\nwebsite: https://northwind.test\n", on("https://northwind.test"), UrlCheck::Same),
        (
            "pw\nurl: https://northwind.example\nurl: https://contoso.example\n",
            on("https://northwind.test"),
            differs("northwind.example", "northwind.test"),
        ),
        // A line that names no host is no evidence either way.
        ("pw\nurl: none\n", on("https://northwind.test"), UrlCheck::NoUrl),
        ("pw\nurl: androidapp://com.northwind\n", on("https://northwind.test"), UrlCheck::NoUrl),
        ("pw\nlogin: you\n", on("https://northwind.test"), UrlCheck::NoUrl),
        ("pw\nhttps://northwind.example\n", on("https://northwind.test"), UrlCheck::NoUrl),
        ("pw\nurl: https://northwind.example\n", vec![], UrlCheck::NoUrl),
        // Several services: the site is any of them.
        (
            "pw\nurl: https://northwind.example\n",
            vec!["https://contoso.example".into(), "northwind.example".into()],
            UrlCheck::Same,
        ),
        // International names compare in ASCII and show by the display rule.
        ("pw\nurl: https://bücher.example\n", on("https://xn--bcher-kva.example"), UrlCheck::Same),
        (
            "pw\nurl: https://paypal.com\n",
            on("https://xn--pypal-4ve.com"),
            differs("paypal.com", "xn--pypal-4ve.com"),
        ),
        (
            "pw\nurl: https://bücher.example\n",
            on("https://bucher.example"),
            differs("bücher.example", "bucher.example"),
        ),
        // CRLF files, as in the fixture.
        ("pw\r\nurl: https://northwind.example\r\n", on("https://northwind.test"), differs("northwind.example", "northwind.test")),
    ];
    for (text, services, want) in cases {
        assert_eq!(check_entry_url(opened(text), services.clone()), want, "{text:?} on {services:?}");
    }
}

#[test]
fn a_closed_entry_has_no_url() {
    let entry = opened("pw\nurl: https://northwind.example\n");
    entry.close();
    assert_eq!(check_entry_url(entry, vec!["https://northwind.test".into()]), UrlCheck::NoUrl);
}

// MARK: The user name

#[test]
fn the_entrys_own_user_name_wins_over_the_path() {
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("pw\nlogin: you@example.com\n", "github.com/alice", Some("you@example.com")),
        ("pw\nuser: bob\n", "github.com/alice", Some("bob")),
        ("pw\nemail: e@example.com\nusername: u\n", "web/github.com", Some("u")),
        ("pw\nLOGIN: Up\n", "web/github.com", Some("Up")),
        ("pw\n", "github.com/alice", Some("alice")),
        ("pw\n", "servers/example.net/root", Some("root")),
        ("pw\n", "bücher.example/alice", Some("alice")),
        ("pw\n", "web/github.com", None),
        ("pw\n", "bank/chase", None),
        ("pw\n", "home/192.168.1.1/admin", None),
        // An empty line is not a user name.
        ("pw\nlogin:\n", "github.com/alice", Some("alice")),
        ("pw\nlogin: \nemail: e@example.com\n", "github.com/alice", Some("e@example.com")),
        ("pw\nlogin:\n", "web/github.com", None),
    ];
    for (text, path, want) in cases {
        assert_eq!(fill_username(opened(text), (*path).into()).as_deref(), *want, "{text:?} at {path}");
    }
}

#[test]
fn path_user_reads_site_user_layouts() {
    let cases: &[(&str, Option<&str>)] = &[
        ("github.com/alice", Some("alice")),
        ("web/github.com/alice", Some("alice")),
        ("web/github.com", None),
        ("alice", None),
        ("contoso/alice", None),
    ];
    for (path, want) in cases {
        assert_eq!(path_user(path).as_deref(), *want, "{path}");
    }
}
