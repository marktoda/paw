//! The store address as typed or pasted: GitHub's short forms, what is
//! tidied, what is refused, and properties over generated input.

use paw_core::sync::address::{AddressNotice, AddressProblem, StoreAddress, read_store_address};
use proptest::prelude::*;

fn github(owner: &str, repo: &str) -> String {
    format!("git@github.com:{owner}/{repo}.git")
}

#[test]
fn githubs_short_forms_become_its_ssh_address_and_say_so() {
    for text in [
        "you/password-store",
        "you/password-store.git",
        "you/password-store/",
        "  you/password-store  ",
        "github.com/you/password-store",
        "GitHub.com/you/password-store.git",
        "www.github.com/you/password-store/",
        "https://github.com/you/password-store",
        "https://github.com/you/password-store.git/",
        "HTTPS://GITHUB.COM/you/password-store",
    ] {
        let parsed = StoreAddress::parse(text).unwrap_or_else(|e| panic!("{text:?}: {e:?}"));
        assert_eq!(parsed.address.display(), github("you", "password-store"), "{text:?}");
        let notice = parsed.notice.unwrap_or_else(|| panic!("{text:?} has no notice"));
        let address = match &notice {
            AddressNotice::GitHubShortForm { address, .. }
            | AddressNotice::GitHubWithoutUser { address }
            | AddressNotice::GitHubWebAddress { address } => address,
            AddressNotice::PortFallback { .. } => panic!("{text:?}: {notice:?}"),
        };
        assert_eq!(*address, github("you", "password-store"), "{text:?}");
    }
}

#[test]
fn a_trailing_slash_is_dropped_from_every_form() {
    for (text, shown) in [
        ("git@github.com:you/store.git/", "git@github.com:you/store.git"),
        ("ssh://git@example.org:2222/srv/store.git/", "ssh://git@example.org:2222/srv/store.git"),
    ] {
        let parsed = StoreAddress::parse(text).unwrap();
        assert_eq!(parsed.address.display(), shown);
        assert_eq!(parsed.notice, None);
    }
}

#[test]
fn the_ffi_reading_names_host_user_and_whether_the_key_ships_with_paw() {
    let reading = read_store_address("you/store".into()).unwrap();
    assert_eq!(reading.address, "git@github.com:you/store.git");
    assert_eq!((reading.user.as_str(), reading.host.as_str(), reading.port), ("git", "github.com", 22));
    assert!(reading.host_key_bundled);
    let short = AddressNotice::GitHubShortForm {
        owner: "you".into(),
        repo: "store".into(),
        address: "git@github.com:you/store.git".into(),
    };
    assert_eq!(reading.notice, Some(short));

    let reading = read_store_address("ssh://me@[::1]:2222/store.git".into()).unwrap();
    assert_eq!((reading.user.as_str(), reading.host.as_str(), reading.port), ("me", "::1", 2222));
    assert!(!reading.host_key_bundled);
    assert_eq!(reading.notice, None);

    assert_eq!(read_store_address("file:///srv/store".into()), None);
}

#[test]
fn ambiguous_or_dangerous_addresses_are_refused() {
    let ssh_address = |host: &str| AddressProblem::NeedsSshAddress { host: host.into() };
    for (text, says) in [
        // Options for ssh or git.
        ("-oProxyCommand=x/store", AddressProblem::NotGit),
        ("you/-store", AddressProblem::NotGit),
        ("-you/store", AddressProblem::NotGit),
        ("git@-host:store", AddressProblem::NotGit),
        ("-git@host:store", AddressProblem::NotGit),
        ("ssh://-oProxyCommand=x@host/store", AddressProblem::NotGit),
        ("ssh://git@-host/store", AddressProblem::NotGit),
        // Other transports.
        ("ext::sh -c touch% /tmp/pwned", AddressProblem::HasSpaces),
        ("ext::ssh", AddressProblem::UseSsh),
        ("fd::17", AddressProblem::UseSsh),
        ("file:///home/you/store", AddressProblem::UseSsh),
        ("git://github.com/you/store.git", AddressProblem::UseSsh),
        // Control characters.
        ("you/store\u{7}", AddressProblem::HasSpaces),
        ("git@github.com:you/st\u{0}ore", AddressProblem::HasSpaces),
        // Not one repository.
        ("you/store/extra", AddressProblem::NotGit),
        ("you/../store", AddressProblem::NotGit),
        ("./store", AddressProblem::NotGit),
        ("you/..", AddressProblem::NotGit),
        ("you.name/store", ssh_address("you.name")),
        ("gitlab.com/you/store", ssh_address("gitlab.com")),
        ("/", AddressProblem::NotGit),
    ] {
        let problem = StoreAddress::parse(text).err().unwrap_or_else(|| panic!("{text:?} was accepted"));
        assert_eq!(problem, says, "{text:?}");
    }
}

#[test]
fn an_address_with_a_secret_in_it_is_refused_without_repeating_it() {
    for text in [
        "https://you:hunter22@github.com/you/store",
        "https://ghp_abcdef0123456789@github.com/you/store.git",
        "ssh://git:hunter22@github.com/you/store.git",
        "git:hunter22@github.com:you/store.git",
    ] {
        let problem = StoreAddress::parse(text).err().unwrap_or_else(|| panic!("{text:?} was accepted"));
        assert_eq!(problem, AddressProblem::HasSecret, "{text:?}");
    }
}

// --- properties -------------------------------------------------------------

fn owner() -> impl Strategy<Value = String> {
    "[A-Za-z0-9]([A-Za-z0-9-]{0,10}[A-Za-z0-9])?"
}

fn repo() -> impl Strategy<Value = String> {
    "[A-Za-z0-9_][A-Za-z0-9._-]{0,20}".prop_filter("not a .git suffix alone", |r| r != "." && !r.ends_with(".git"))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Every way of writing one GitHub repository means the same address.
    #[test]
    fn every_github_form_means_the_same_address(
        owner in owner(),
        repo in repo(),
        git in any::<bool>(),
        slash in any::<bool>(),
        form in 0usize..5,
        before in "[ \t]{0,2}",
        after in "[ \t\n]{0,2}",
    ) {
        let name = format!("{owner}/{repo}{}", if git { ".git" } else { "" });
        let written = match form {
            0 => name.clone(),
            1 => format!("github.com/{name}"),
            2 => format!("https://github.com/{name}"),
            3 => format!("git@github.com:{owner}/{repo}.git"),
            _ => format!("ssh://git@github.com/{owner}/{repo}.git"),
        };
        let text = format!("{before}{written}{}{after}", if slash { "/" } else { "" });
        let parsed = StoreAddress::parse(&text).map_err(|e| TestCaseError::fail(format!("{text:?}: {e:?}")))?;
        prop_assert_eq!(parsed.address.host.as_str(), "github.com");
        prop_assert_eq!(parsed.address.user.as_str(), "git");
        prop_assert_eq!(parsed.address.port, 22);
        prop_assert_eq!(parsed.address.path.trim_start_matches('/'), format!("{owner}/{repo}.git"));
        prop_assert_eq!(parsed.notice.is_some(), form < 3);
    }

    /// Whatever is accepted is safe to hand to the SSH connection, and what
    /// is saved reads back the same.
    #[test]
    fn whatever_is_accepted_is_safe_and_reads_back_the_same(text in any::<String>()) {
        check_accepted(&text)?;
    }

    /// The same, for text shaped like addresses, where acceptance is likely.
    #[test]
    fn address_shaped_text_that_is_accepted_is_safe(
        scheme in prop::sample::select(vec!["", "ssh://", "https://", "http://", "ext::", "file://", "SSH://"]),
        user in "[-a-z@:]{0,6}",
        at in prop::sample::select(vec!["", "@"]),
        host in "[-a-z0-9.\\[\\]:]{0,12}",
        separator in prop::sample::select(vec!["", ":", "/", ":/", "::"]),
        path in "[-a-z0-9./~'\"\\\\:@ ]{0,16}",
    ) {
        check_accepted(&format!("{scheme}{user}{at}{host}{separator}{path}"))?;
    }

    /// Control characters anywhere are refused.
    #[test]
    fn control_characters_are_refused(prefix in "[a-z/@:]{0,8}", control in "[\\x00-\\x1f\\x7f]", suffix in "[a-z/]{1,8}") {
        let text = format!("git@github.com:{prefix}{control}{suffix}");
        prop_assert!(StoreAddress::parse(&text).is_err(), "{:?} was accepted", text);
    }

    /// A password or token in a web address is never accepted or repeated.
    #[test]
    fn a_secret_is_never_accepted_or_repeated(user in "[a-z]{1,6}", secret in "[A-Za-z0-9_]{12,20}") {
        for text in [
            format!("https://{user}:{secret}@github.com/you/store"),
            format!("https://{secret}@github.com/you/store"),
            format!("ssh://{user}:{secret}@github.com/you/store.git"),
            format!("{user}:{secret}@github.com:you/store.git"),
        ] {
            let refused = StoreAddress::parse(&text);
            prop_assert_eq!(refused, Err(AddressProblem::HasSecret), "{:?}", text);
        }
    }
}

fn check_accepted(text: &str) -> Result<(), TestCaseError> {
    let Ok(parsed) = StoreAddress::parse(text) else { return Ok(()) };
    let address = &parsed.address;
    for part in [&address.user, &address.host, &address.path] {
        prop_assert!(!part.starts_with('-'), "{:?} gave {:?}", text, address);
        prop_assert!(!part.chars().any(|c| c.is_control() || c.is_whitespace()), "{:?} gave {:?}", text, address);
    }
    prop_assert!(!address.user.is_empty() && !address.host.is_empty(), "{:?} gave {:?}", text, address);
    prop_assert!(!address.user.contains([':', '@']), "{:?} gave {:?}", text, address);
    prop_assert!(!address.host.contains(['@', '/']), "{:?} gave {:?}", text, address);
    prop_assert!(!address.path.trim_start_matches('/').is_empty(), "{:?} gave {:?}", text, address);
    prop_assert!(address.port > 0);
    let again = StoreAddress::parse(&address.display()).map_err(|e| TestCaseError::fail(format!("{text:?}: {e:?}")))?;
    prop_assert_eq!(&again.address, address, "{:?}", text);
    prop_assert_eq!(again.notice, None);
    Ok(())
}
