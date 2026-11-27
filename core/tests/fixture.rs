//! The fixture store the simulator runs on, driven through the same
//! objects the app uses: open the store, browse, search, ask who can open
//! an entry, and open one with the (simulated) key through a key task.

#![cfg(feature = "sim")]

mod key_common;

use std::path::Path;
use std::sync::Arc;

use key_common::{Driver, PIN, finish, is_touch};
use paw_core::entry::opened::OpenedEntry;
use paw_core::key::{KeyError, KeyPort};
use paw_core::task::{KeyOutcome, KeyTaskState};
use paw_core::recipients::{Access, Recipient};
use paw_core::sim::SimulatedKey;
use paw_core::sim::copies::SimClock;
use paw_core::sim::fixture::{
    OTHER_KEY_FOLDER, encrypt, other_key, other_key_fingerprint_hex, write_fixture_store, write_name_match_entries,
};
use paw_core::store::service::{MatchKind, NamePairing};
use paw_core::store::PassStore;
use paw_core::PawError;

/// A simulated key plugged into a key port, as the app holds one.
struct Plugged {
    key: Arc<SimulatedKey>,
    port: Arc<KeyPort>,
}

/// How one open went: the entry or the task's error, and how often the
/// task asked for a touch.
struct Opened {
    result: Result<Arc<OpenedEntry>, KeyError>,
    touches: usize,
}

impl Plugged {
    /// Opens `entry` from `store` as the app does: a key task, which types
    /// `pin` and touches the key whenever it asks.
    fn open(&self, store: &Arc<PassStore>, pin: &str, entry: &str) -> Opened {
        let driver = Driver::helpful(pin, &self.key);
        let task = self.port.clone().open_entry(store.clone(), entry.into(), None, 0, driver.clone());
        driver.drive(&task);
        let result = match finish(&task) {
            KeyTaskState::Done { outcome: KeyOutcome::Opened { entry, .. } } => Ok(entry),
            KeyTaskState::Failed { error } => Err(error),
            other => panic!("{entry}: the task ended {other:?}"),
        };
        Opened { result, touches: driver.count(is_touch) }
    }
}

fn plugged_key(key: Arc<SimulatedKey>) -> Plugged {
    let port = KeyPort::new(SimClock::new(1_790_000_000_000));
    port.plugged_in(key.clone()).unwrap();
    Plugged { key, port }
}

/// A key whose slots ask for a touch every time.
fn plugged() -> Plugged {
    plugged_key(SimulatedKey::new(PIN.into(), true, 5_000))
}

/// The fixture store in a state directory; returns its working copy too.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Arc<PassStore>) {
    let state = tempfile::tempdir().unwrap();
    let repo = state.path().join(paw_core::files::layout::REPO);
    write_fixture_store(repo.to_str().unwrap().into()).unwrap();
    let store = PassStore::open(state.path().to_str().unwrap().into()).unwrap();
    (state, repo, store)
}

/// Every file under `dir`, by relative path.
fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for child in std::fs::read_dir(dir).unwrap() {
            let path = child.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let name = path.strip_prefix(root).unwrap().to_str().unwrap().to_owned();
                out.push((name, std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn writing_the_fixture_twice_gives_the_same_bytes() {
    let (_first_state, first, _) = fixture();
    let (_second_state, second, _) = fixture();
    assert_eq!(files(&first), files(&second));

    // Writing over an existing copy changes nothing either.
    let before = files(&first);
    write_fixture_store(first.to_str().unwrap().into()).unwrap();
    assert_eq!(files(&first), before);
}

#[test]
fn the_card_can_open_everything_but_the_archive() {
    let (_state, _, store) = fixture();
    let card = plugged().port.session().unwrap().info().decryption_fingerprint.unwrap();
    let other = other_key_fingerprint_hex().unwrap();
    let other_id = Recipient::KeyId { id: other[other.len() - 16..].to_owned() };

    for entry in store.entries() {
        let access = store.access(entry.path.clone(), card.clone());
        if entry.folder == OTHER_KEY_FOLDER {
            assert_eq!(access, Access::NotForThisKey { recipients: vec![other_id.clone()] }, "{}", entry.path);
            let found = paw_core::recipients::entry_recipients(&store.read_entry(&entry.path).unwrap()).unwrap();
            assert_eq!(found.keys, std::slice::from_ref(&other_id));
        } else {
            assert_eq!(access, Access::CanOpen, "{}", entry.path);
        }
    }
    assert!(matches!(store.access("web/no-such-entry".into(), card), Access::Unreadable { .. }));
}

#[test]
fn opening_an_entry_takes_the_pin_and_one_touch() {
    let (_state, _, store) = fixture();
    let plugged = plugged();
    assert!(!plugged.port.session().unwrap().holds_pin());

    let done = plugged.open(&store, PIN, "web/github.com");
    let opened = done.result.unwrap();

    assert_eq!(opened.password().as_deref(), Some("tidal-Quartz-88-ferns"));
    assert_eq!(opened.username().as_deref(), Some("you@example.com"));
    assert_eq!(opened.url().as_deref(), Some("https://github.com/login"));
    assert_eq!(opened.notes().as_deref(), Some("recovery codes are in the blue notebook"));
    assert!(plugged.port.session().unwrap().holds_pin(), "the PIN typed is held for the next open");
    assert_eq!(done.touches, 1);
    assert_eq!(plugged.key.decipher_count(), 1);
}

#[test]
fn every_fixture_entry_for_the_card_opens_as_text() {
    let (_state, _, store) = fixture();
    let plugged = plugged();

    let mut opened = 0;
    for entry in store.entries().iter().filter(|e| e.folder != OTHER_KEY_FOLDER) {
        let doc = plugged.open(&store, PIN, &entry.path).result.unwrap();
        assert!(doc.view().has_password, "{}", entry.path);
        opened += 1;
    }
    assert_eq!(opened, 27);
    assert_eq!(plugged.key.decipher_count(), 27);
}

#[test]
fn an_entry_for_another_key_is_refused_before_the_pin_or_a_touch() {
    let (_state, _, store) = fixture();
    let plugged = plugged();

    // Even a wrong PIN is not sent: the refusal comes first.
    for pin in [PIN, "000000"] {
        let done = plugged.open(&store, pin, "archive/old-forum");
        match done.result {
            Err(KeyError::NotForThisKey { recipients }) => {
                let other = other_key_fingerprint_hex().unwrap();
                assert_eq!(recipients, [Recipient::KeyId { id: other[other.len() - 16..].to_owned() }]);
            }
            other => panic!("expected NotForThisKey, got {:?}", other.map(|_| "an opened entry")),
        }
        assert_eq!(done.touches, 0);
    }
    assert_eq!(plugged.key.decipher_count(), 0);
    assert_eq!(plugged.key.pin_tries_left(), 3);
}

#[test]
fn an_entry_for_several_keys_costs_one_touch_wherever_the_card_is_listed() {
    let plugged = plugged();
    let on_card = plugged.key.decryption_key().unwrap();
    let other = other_key().unwrap();
    let state = tempfile::tempdir().unwrap();
    let dir = state.path().join(paw_core::files::layout::REPO);
    std::fs::create_dir_all(&dir).unwrap();

    for (name, recipients) in [("card-last", vec![other.clone(), on_card.clone()]), ("card-first", vec![on_card.clone(), other.clone()])] {
        let entry = encrypt(&recipients, b"shared\n").unwrap();
        std::fs::write(dir.join(format!("{name}.gpg")), entry).unwrap();
        let store = PassStore::open(state.path().to_str().unwrap().into()).unwrap();
        let before = plugged.key.decipher_count();

        let done = plugged.open(&store, PIN, name);

        assert_eq!(done.result.unwrap().password().as_deref(), Some("shared"), "{name}");
        assert_eq!(done.touches, 1, "{name}");
        assert_eq!(plugged.key.decipher_count(), before + 1, "{name}");
    }
}

#[test]
fn a_missing_or_unsafe_entry_is_refused_without_touching_the_card() {
    let (_state, _, store) = fixture();
    let plugged = plugged();

    for (entry, unsafe_path) in [("web/nothing-here", false), ("../elsewhere", true), (".git/config", true)] {
        let refused = plugged.open(&store, PIN, entry).result.err().unwrap();
        assert!(matches!(refused, KeyError::EntryUnreadable { .. }), "{entry}: {refused:?}");
        // Why, as the store says it.
        let why = store.read_entry(entry).err().unwrap();
        assert_eq!(matches!(why, PawError::UnsafePath(_)), unsafe_path, "{entry}: {why:?}");
        assert_eq!(matches!(why, PawError::Store(_)), !unsafe_path, "{entry}: {why:?}");
    }
    assert_eq!(plugged.key.decipher_count(), 0);
    assert!(!plugged.port.session().unwrap().holds_pin(), "nothing asked for the PIN");
}

#[test]
fn the_view_of_an_opened_entry_holds_no_secret_values() {
    let (_state, _, store) = fixture();
    let opened = plugged().open(&store, PIN, "bank/first-example-bank").result.unwrap();

    let view = opened.view();
    assert!(view.has_password && view.has_username && view.has_url && view.has_notes);
    assert!(!view.has_otp);
    let fields: Vec<_> = view.fields.iter().map(|f| (f.key.as_str(), f.starts_hidden)).collect();
    assert_eq!(fields, [("security answer", true), ("card pin", true)]);

    let printed = format!("{view:?}");
    for secret in ["Harbor_73_willow", "00417788", "Biscuit", "2580", "bank.example", "Fridays"] {
        assert!(!printed.contains(secret), "{printed}");
    }

    // Values come one at a time, by the id the view gave.
    assert_eq!(opened.field_value(view.fields[0].id).as_deref(), Some("Biscuit"));
    assert_eq!(opened.field_value(view.fields[1].id).as_deref(), Some("2580"));
    assert_eq!(opened.field_value(0), None, "the password line is not a field");
    assert_eq!(opened.field_value(999), None);
}

#[test]
fn a_one_time_code_secret_is_flagged_and_never_handed_out() {
    let (_state, _, store) = fixture();
    let opened = plugged().open(&store, PIN, "web/github.com").result.unwrap();

    let view = opened.view();
    assert!(view.has_otp);
    assert!(view.fields.is_empty());
    assert!(!opened.notes().unwrap().contains("JBSWY3DPEHPK3PXP"));
}

#[test]
fn closing_an_opened_entry_takes_everything_away() {
    let opened = OpenedEntry::decrypted("x", b"pw\nlogin: you\nurl: example.com\nport: 22\nnote\n".to_vec(), vec![0; 32]).unwrap();
    let id = opened.view().fields[0].id;
    assert_eq!(opened.field_value(id).as_deref(), Some("22"));

    opened.close();

    assert_eq!(opened.password(), None);
    assert_eq!(opened.username(), None);
    assert_eq!(opened.url(), None);
    assert_eq!(opened.notes(), None);
    assert_eq!(opened.field_value(id), None);
    let view = opened.view();
    assert!(!view.has_password && !view.has_username && !view.has_url && !view.has_notes);
    assert!(view.fields.is_empty());
}

#[test]
fn an_entry_that_is_not_text_opens_as_an_error() {
    let key = SimulatedKey::new(PIN.into(), false, 50);
    let state = tempfile::tempdir().unwrap();
    let dir = state.path().join(paw_core::files::layout::REPO);
    std::fs::create_dir_all(&dir).unwrap();
    let binary = encrypt(&[key.decryption_key().unwrap()], &[0xFF, 0xD8, 0xFF, 0xE0, 0x00]).unwrap();
    std::fs::write(dir.join("photo.gpg"), binary).unwrap();
    let store = PassStore::open(state.path().to_str().unwrap().into()).unwrap();
    let refused = plugged_key(key).open(&store, PIN, "photo").result.err().unwrap();
    assert_eq!(refused, KeyError::EntryUnreadable { detail: "it is not text".into() });
    assert!(matches!(OpenedEntry::decrypted("x", vec![0xC3, 0x28], vec![0; 32]), Err(PawError::NotText)));
}

#[test]
fn the_harness_adds_entries_named_without_a_domain_that_open_and_match_by_name() {
    let (state, repo, _) = fixture();
    write_name_match_entries(repo.to_str().unwrap().into()).unwrap();
    let store = PassStore::open(state.path().to_str().unwrap().into()).unwrap();
    let plugged = plugged();
    for entry in ["finance/northwind", "contoso/alice"] {
        let opened = plugged.open(&store, PIN, entry).result.unwrap();
        assert!(opened.view().has_password, "{entry}");
    }
    let found: Vec<_> = store
        .match_services(vec!["https://www.northwind.example/login".into()], None)
        .into_iter()
        .map(|m| (m.entry.path, m.kind, m.site))
        .collect();
    assert_eq!(
        found,
        [(
            "finance/northwind".to_owned(),
            MatchKind::Name { pairing: NamePairing::Unpaired },
            "northwind.example".to_owned()
        )]
    );
}

#[test]
fn the_sites_the_autofill_tests_use_gain_no_name_matches_from_the_fixture() {
    let (_state, _, store) = fixture();
    let sites = [
        "https://github.com/login",
        "https://github.com",
        "https://example.com",
        "https://github.com.attacker.example/login",
        "https://nothing.example",
        "https://paypa1.example",
    ];
    for site in sites {
        let found = store.match_services(vec![site.into()], None);
        assert!(found.iter().all(|m| !matches!(m.kind, MatchKind::Name { .. })), "{site}");
    }
}
