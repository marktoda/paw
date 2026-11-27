//! Every kind of state file, as an installed store has it on disk: each
//! still reads, and what paw writes is byte for byte what it wrote before.

use std::path::Path;

use super::layout;

const FINGERPRINT: &str = "0123456789ABCDEF0123456789ABCDEF01234567";
const SSH_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";

fn state() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn put(dir: &Path, name: &str, bytes: &[u8]) {
    std::fs::write(dir.join(name), bytes).unwrap();
}

fn get(dir: &Path, name: &str) -> Vec<u8> {
    std::fs::read(dir.join(name)).unwrap()
}

#[test]
fn the_enrolled_key() {
    use crate::key::enrolment::{load_enrolment, save_enrolment};
    let bytes = format!("ident=0006:12345678\ndecryption_fingerprint={FINGERPRINT}\nauthentication_fingerprint=AB12\n");
    let dir = state();
    put(dir.path(), layout::ENROLLED_KEY, bytes.as_bytes());
    let key = load_enrolment(dir.path()).unwrap().unwrap();
    assert_eq!(key.ident, "0006:12345678");
    assert_eq!(key.decryption_fingerprint.as_deref(), Some(FINGERPRINT));
    assert_eq!(key.authentication_fingerprint.as_deref(), Some("AB12"));

    let again = state();
    save_enrolment(again.path(), &key).unwrap();
    assert_eq!(get(again.path(), layout::ENROLLED_KEY), bytes.as_bytes());
}

#[test]
fn the_setup_step() {
    use crate::setup::{SetupFlow, SetupState};
    let bytes = format!(
        "step=check-host\nident=0006:12345678\nssh_public_key={SSH_KEY}\ndecryption_fingerprint={FINGERPRINT}\n\
         authentication_touch=cached\ndecryption_touch=each-use\naddress=ssh://git@example.com/you/store.git\n\
         notice=port-fallback github.com 22 ssh.github.com 443\n"
    );
    let dir = state();
    put(dir.path(), layout::SETUP, bytes.as_bytes());
    let mut flow = SetupFlow::open(dir.path()).unwrap();
    assert_eq!(
        flow.state(),
        SetupState::CheckHost {
            address: "ssh://git@example.com/you/store.git".into(),
            notice: Some(crate::sync::address::AddressNotice::PortFallback {
                fallback: crate::remote::PortFallback {
                    from_host: "github.com".into(),
                    from_port: 22,
                    host: "ssh.github.com".into(),
                    port: 443,
                },
            }),
        }
    );
    assert!(!layout::is_set_up(dir.path()));

    // Setting the same address saves the file again, as it was.
    flow.set_address("ssh://git@example.com/you/store.git").unwrap();
    let written = String::from_utf8(get(dir.path(), layout::SETUP)).unwrap();
    assert_eq!(written, bytes.replace("notice=port-fallback github.com 22 ssh.github.com 443\n", ""));
}

#[test]
fn a_setup_step_with_the_notice_an_earlier_paw_saved_in_words() {
    use crate::setup::{SetupFlow, SetupState};
    let bytes = format!(
        "step=check-host\nident=0006:12345678\nssh_public_key={SSH_KEY}\ndecryption_fingerprint={FINGERPRINT}\n\
         authentication_touch=cached\ndecryption_touch=each-use\naddress=ssh://git@ssh.github.com:443/you/store.git\n\
         notice=Port 22 of github.com could not be reached, so paw will use ssh.github.com on port 443 instead.\n"
    );
    let dir = state();
    put(dir.path(), layout::SETUP, bytes.as_bytes());
    let flow = SetupFlow::open(dir.path()).unwrap();
    assert_eq!(
        flow.state(),
        SetupState::CheckHost { address: "ssh://git@ssh.github.com:443/you/store.git".into(), notice: None }
    );
}

#[test]
fn a_finished_setup() {
    let dir = state();
    put(dir.path(), layout::SETUP, b"step=done\nident=0006:12345678\n");
    std::fs::create_dir_all(layout::repo_dir(dir.path())).unwrap();
    put(&layout::repo_dir(dir.path()), ".gpg-id", b"AAAA1111\n");
    assert!(layout::is_set_up(dir.path()));
}

#[test]
fn the_remote() {
    let dir = state();
    put(dir.path(), layout::REMOTE, b"ssh://git@example.com/you/store.git\n");
    let address = crate::sync::address::saved_address(dir.path()).unwrap();
    assert_eq!(address.display(), "ssh://git@example.com/you/store.git");
}

#[test]
fn the_github_link() {
    use crate::github::refresh::GitHubLink;
    // Written before with the repository's `id`, which is no longer kept.
    let older = b"repository=you/store\nid=42\nssh=git@github.com:you/store.git\n";
    let bytes = b"repository=you/store\nssh=git@github.com:you/store.git\n";
    let dir = state();
    put(dir.path(), layout::GITHUB_LINK, older);
    let link = GitHubLink::load(dir.path()).unwrap();
    assert_eq!(link, GitHubLink { full_name: "you/store".into(), ssh_address: "git@github.com:you/store.git".into() });

    let again = state();
    link.save(again.path()).unwrap();
    assert_eq!(get(again.path(), layout::GITHUB_LINK), bytes);
    assert_eq!(GitHubLink::load(again.path()), Some(link));
}

#[test]
fn the_known_hosts() {
    use crate::sync::hosts::KnownHosts;
    let bytes = format!("git.example.com 22 {SSH_KEY}\n");
    let dir = state();
    put(dir.path(), layout::KNOWN_HOSTS, bytes.as_bytes());
    let hosts = KnownHosts::of(dir.path());
    assert!(hosts.lookup("git.example.com", 22).unwrap().is_some());

    let again = state();
    KnownHosts::of(again.path()).add("git.example.com", 22, SSH_KEY).unwrap();
    assert_eq!(get(again.path(), layout::KNOWN_HOSTS), bytes.as_bytes());
}

/// A card key line as every paw has written it: the fields for a key from
/// a certificate are `-`.
const CARD_KEY_LINE: &str = "key 678417EACF97C347CEF3717187853EFA4A0E0CFF card - - - - \
    xjgEZVPxABIKKwYBBAGXVQEFAQEHQFfbSzWfI65eFG5OJRIFZwRyJQY0jBUMFHU9DJM9BNQhAwEIBw==\n";
const CARD_KEY: &str = "678417EACF97C347CEF3717187853EFA4A0E0CFF";

fn pinned_recipients(keys: &str) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    use sha2::{Digest, Sha256};
    let gpg_id = "AAAA1111\n";
    let sha: String = Sha256::digest(gpg_id.as_bytes()).iter().map(|b| format!("{b:02X}")).collect();
    format!(
        "# paw pinned recipients, version 1\n{keys}bind {CARD_KEY} {}\naccepted\ngpgid {sha} {} {}\n",
        B64.encode("AAAA1111"),
        B64.encode(".gpg-id"),
        B64.encode(gpg_id),
    )
}

fn store_with_recipients(bytes: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = state();
    put(dir.path(), layout::RECIPIENTS, bytes.as_bytes());
    let repo = layout::repo_dir(dir.path());
    std::fs::create_dir_all(&repo).unwrap();
    put(&repo, ".gpg-id", b"AAAA1111\n");
    (dir, repo)
}

#[test]
fn the_pinned_recipients() {
    let bytes = pinned_recipients(CARD_KEY_LINE);
    let (dir, repo) = store_with_recipients(&bytes);
    let mut keyring = crate::write::Keyring::open(dir.path()).unwrap();
    let folder = keyring.folder_recipients(&repo, "").unwrap();
    assert!(folder.confirmed);
    assert!(!folder.changed);
    assert!(folder.writable);
    assert_eq!(folder.accepted_lines, ["AAAA1111"]);
    assert_eq!(folder.lines[0].status, crate::write::LineStatus::Ready { fingerprint: CARD_KEY.into(), bound: true });

    // Pinning the card's key again saves the file again, as it was.
    let held = keyring.lock().exclusive().unwrap();
    let key = keyring.keys()[0].key.clone();
    keyring.pin_card_key(&held, &key).unwrap();
    assert_eq!(get(dir.path(), layout::RECIPIENTS), bytes.as_bytes());
}

#[test]
fn pinned_recipients_with_a_key_from_a_certificate_still_read() {
    // What an earlier paw's certificate import wrote; no phone has it, but
    // the file must not stop reading if one did.
    let certificate = "key 0123456789ABCDEF0123456789ABCDEF01234567 certificate \
        FEDCBA9876543210FEDCBA9876543210FEDCBA98 1800000000 FEDCBA9876543210FEDCBA9876543210FEDCBA98 \
        team@example.com AAAA\nsub 0123456789ABCDEF0123456789ABCDEF01234567 \
        1111222233334444555566667777888899990000 cmV2b2tlZA==\n";
    let (dir, repo) = store_with_recipients(&pinned_recipients(&format!("{CARD_KEY_LINE}{certificate}")));
    let keyring = crate::write::Keyring::open(dir.path()).unwrap();
    assert_eq!(keyring.keys().iter().map(|k| k.fingerprint.as_str()).collect::<Vec<_>>(), [CARD_KEY]);
    assert!(keyring.folder_recipients(&repo, "").unwrap().writable);
}

#[test]
fn the_site_pairings() {
    use crate::store::pairings::{Pairing, PairingFile};
    let bytes = br#"{"version":1,"pairings":[{"entry":"bank","sites":["bank.com"]}]}"#;
    let dir = state();
    put(dir.path(), layout::SITE_PAIRINGS, bytes);
    let file = PairingFile::new(dir.path());
    assert_eq!(file.load(), [Pairing { entry: "bank".into(), sites: vec!["bank.com".into()] }]);

    file.record("bank", "bank.com", None).unwrap();
    assert_eq!(get(dir.path(), layout::SITE_PAIRINGS), bytes);
}

#[test]
fn the_one_time_code_list() {
    use crate::store::codes::CodeFile;
    let dir = state();
    put(dir.path(), layout::ONE_TIME_CODES_ON, b"");
    put(dir.path(), layout::ONE_TIME_CODES, br#"{"version":1,"entries":["web/github.com"]}"#);
    let file = CodeFile::new(dir.path());
    assert!(file.enabled());
    assert_eq!(file.load(), ["web/github.com"]);

    file.mark("mail", true).unwrap();
    assert_eq!(get(dir.path(), layout::ONE_TIME_CODES), br#"{"version":1,"entries":["web/github.com","mail"]}"#);
}

#[test]
fn the_key_log() {
    let dir = state();
    put(dir.path(), &format!("{}fill", layout::KEY_LOG_PREFIX), b"1000\tkey plugged in\n");
    let lines = crate::key::key_log(dir.path().to_string_lossy().into_owned(), 0);
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].at_ms, lines[0].process.as_str(), lines[0].text.as_str()), (1000, "fill", "key plugged in"));
}

