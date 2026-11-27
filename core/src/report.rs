//! Settings › Key diagnostics › Copy diagnostics: one block of plain text
//! for a bug report, pasted on a desktop.
//!
//! Never in it: entry, folder, host, repository or user names, the store's
//! address, any fingerprint or key ID, the key's serial number, a PIN, a
//! token or anything decrypted. `diagnostics_say_what_they_should_and_name_nothing`
//! in `core/tests/settings.rs` checks that.

use std::path::Path;
use std::sync::Arc;

use crate::key::KeyTouch;
use crate::copies::CopyCache;
use crate::github::GitHubAccount;
use crate::key::KeyPort;
use crate::store::tree::StoreTree;

/// The last sync, as the pill on Home last said it. A fixed word, so no
/// text from git or the server can reach the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LastSyncState {
    NotChecked,
    Working,
    UpToDate,
    NotSent,
    Offline,
    Failed,
    /// Stopped until the person looks (replaced history, refused or held
    /// back changes).
    Paused,
    NeedsKey,
}

/// What only the app knows.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DiagnosticsFacts {
    pub app_version: String,
    pub app_build: String,
    /// e.g. "iOS 26.1".
    pub system_version: String,
    /// The model identifier, e.g. "iPhone17,1".
    pub device_model: String,
    pub last_sync: Option<LastSyncState>,
    /// When paw last checked for changes, milliseconds since 1970.
    pub last_checked_ms: Option<i64>,
}

fn sync_words(summary: LastSyncState) -> &'static str {
    match summary {
        LastSyncState::NotChecked => "not checked yet",
        LastSyncState::Working => "working",
        LastSyncState::UpToDate => "up to date",
        LastSyncState::NotSent => "changes not sent",
        LastSyncState::Offline => "offline",
        LastSyncState::Failed => "failed",
        LastSyncState::Paused => "paused",
        LastSyncState::NeedsKey => "needs the key",
    }
}

fn touch_words(touch: KeyTouch) -> &'static str {
    match touch {
        KeyTouch::Off => "never",
        KeyTouch::EachUse => "every time",
        KeyTouch::Cached => "once per 15 seconds",
        KeyTouch::Unknown => "unknown",
    }
}

fn utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let in_day = secs.rem_euclid(86_400);
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", in_day / 3600, in_day / 60 % 60)
}

/// Keeps only what a version string needs, so the app cannot pass a name
/// through by mistake.
fn plain(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ',' | ' ' | '-' | '(' | ')'))
        .take(40)
        .collect();
    if kept.trim().is_empty() { "unknown".into() } else { kept.trim().to_owned() }
}

fn store_lines(state_dir: Option<&Path>, out: &mut Vec<String>) {
    let set_up = state_dir.is_some_and(crate::setup::is_set_up);
    out.push(format!("Store: {}", if set_up { "set up" } else { "not set up" }));
    let Some(state_dir) = state_dir else { return };
    let repo = crate::files::layout::repo_dir(state_dir);
    match StoreTree::scan(&repo) {
        Ok(tree) => out.push(format!("Passwords: {} in {} folders", tree.entries().len(), tree.folders().len())),
        Err(_) => out.push("Passwords: unreadable".into()),
    }
    match crate::sync::queue::unpushed(&crate::files::layout::StoreDirs::new(state_dir)) {
        Ok(items) => out.push(format!("Changes not sent: {}", items.len())),
        Err(_) => out.push("Changes not sent: unknown".into()),
    }
}

/// The report. `state_dir` is the store's (none before setup); the key
/// log is read from where `port` writes it.
#[uniffi::export]
pub fn diagnostics_text(
    facts: DiagnosticsFacts,
    state_dir: Option<String>,
    port: Arc<KeyPort>,
    github: Option<Arc<GitHubAccount>>,
    copies: Option<Arc<CopyCache>>,
) -> String {
    let mut out = vec![
        "paw diagnostics".to_owned(),
        format!("App: {} ({})", plain(&facts.app_version), plain(&facts.app_build)),
        format!("System: {}", plain(&facts.system_version)),
        format!("Device: {}", plain(&facts.device_model)),
    ];

    store_lines(state_dir.as_deref().map(Path::new), &mut out);
    let checked = facts.last_checked_ms.map(|ms| format!(", checked {}", utc(ms.div_euclid(1000))));
    match facts.last_sync {
        Some(summary) => out.push(format!("Last sync: {}{}", sync_words(summary), checked.unwrap_or_default())),
        None => out.push("Last sync: no sync in this store".into()),
    }

    out.push(match &copies {
        None => "Face ID copies: not on this iPhone".into(),
        Some(copies) if !copies.copies_enabled() => "Face ID copies: off".into(),
        Some(copies) => match copies.list() {
            Ok(live) => format!("Face ID copies: on, {} open", live.len()),
            Err(_) => "Face ID copies: on, unreadable".into(),
        },
    });

    out.push(match &github {
        None => "GitHub: no sign-in in this build".into(),
        Some(github) => match github.sign_in_expiry() {
            Ok(None) => "GitHub: not signed in".into(),
            Ok(Some(expiry)) => {
                let when = |at: Option<u64>| at.map_or("no end given".into(), |s| utc(s as i64));
                format!(
                    "GitHub: signed in; access until {}; renewal until {}",
                    when(expiry.access_until_secs),
                    when(expiry.renewal_until_secs)
                )
            }
            Err(_) => "GitHub: sign-in unreadable".into(),
        },
    });

    match port.session() {
        None => out.push("Key: not plugged in".into()),
        Some(session) => {
            let info = session.info();
            let kdf = session.pin_kdf();
            out.push("Key: plugged in".into());
            let ours = if port.key_matches_enrolment() { "yes" } else { "no" };
            out.push(format!("  The store's key: {ours}"));
            out.push(format!("  PIN tries left: {}", info.pin_tries_left));
            out.push(format!("  Touch to open: {}", touch_words(info.decryption_touch)));
            out.push(format!("  Touch to sign in: {}", touch_words(info.authentication_touch)));
            let kdf = match (kdf.enabled, kdf.supported) {
                (false, _) => "off",
                (true, true) => "on",
                (true, false) => "on, not supported",
            };
            out.push(format!("  PIN setting (KDF): {kdf}"));
        }
    }

    out.push(String::new());
    out.push("Key log (UTC):".into());
    let log = port.key_log_text(0);
    if log.is_empty() {
        out.push("(empty)".into());
    }
    let mut text = out.join("\n");
    text.push('\n');
    text.push_str(&log);
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(1_795_520_000), "2026-11-24 11:33 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00 UTC");
    }

    #[test]
    fn versions_lose_anything_else() {
        assert_eq!(plain("1.0 (42)"), "1.0 (42)");
        assert_eq!(plain("iOS 26.1"), "iOS 26.1");
        assert_eq!(plain("me@example.com/secret"), "meexample.comsecret");
        assert_eq!(plain("   "), "unknown");
    }
}
