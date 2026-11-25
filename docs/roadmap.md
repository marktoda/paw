# paw roadmap

What is done, what is left, and what is being worked on. Kept current as work lands. The reasoning behind each milestone is in [design.md](design.md) section 7; the look is in [design-language.md](design-language.md); the rules are in [principles.md](principles.md).

Status words: **done** (on main, green on GitHub), **in progress** (a branch exists), **next** (unblocked, not started), **blocked** (waits on something named), **later**.

## How the work is run

- Building is done by subagents, each on its own branch in `.worktrees/<branch>`, one track per area so branches do not collide. Each branch is reviewed, built on GitHub, then merged.
- Rust core work is tested on Linux in minutes. Swift work compiles only on GitHub: a full run is about 30 minutes across five machines when the queue is empty, a narrowed run about 15; GitHub runs about five Mac jobs at once for the account, so parallel tracks queue. Tracks are therefore cut so that as much as possible lands in the core.
- Anything that needs the owner's iPhone and YubiKey is collected under "Needs the device" so those checks can be done in one sitting.

## Proven so far

| What | Status |
|---|---|
| Build, sign and install with no Mac (GitHub builds, signed and installed from Linux over Wi-Fi) | done |
| Decrypt with the YubiKey over USB-C in the app and in the AutoFill extension (iPhone, iOS 27.2.1) | done |
| Card answers PIN state queries; login-sized signatures work in every encoding; auth slot caches a touch for 15 s | done |
| Clone, commit, fetch, push over SSH with the login signed by a simulated card | done |
| Read a store: tree, `.gpg-id`, who can open an entry, entry text, search, site matching, fixture store | done |
| Design language, logo candidates, SwiftUI tokens, components and gallery | done |
| Face ID copies in the core: per-entry deadlines, tamper and clock rules, simulated keystore and clock | done |
| Staying in step with a remote: replay, keep both, unpushed queue, setup flow, pinned GitHub host keys | done |

## Milestones

### M1 Foundations and key session

| Item | Track | Status |
|---|---|---|
| Raw card checks on the test screen | core + app | done; the test screen and its PIN-taking checks are gone (branch `release-build`); the touch timeout check is in Settings › Key diagnostics, as a key task |
| Exact touch policy per slot | core | done |
| Keep the simulated key and fixture out of device builds (cargo feature, device-library scan in CI) | foundations | done on branch `release-build`: the `device` job builds Release against the core without `sim` and scans the library, the app and the extension; never run on the phone |
| App Group container and shared keychain group for app and extension | foundations | done; the store and Face ID copies live there |
| Key session in the core: one object per plugged-in key, PIN held for the session and forgotten after 15 minutes in the background, cancel, structured errors | key-session | done |
| Key task state machine (plug in, PIN, touch, working, done) driving the real key sheet | key-session, then screens | done |
| Touch-timeout status word from the real key | device | blocked on the owner running one check |

### M2 Read a store

| Item | Track | Status |
|---|---|---|
| Store model in the core | core | done |
| Home, folder, search and opened entry screens on the fixture store; copy with expiry, reveal, large type, app-switcher cover | screens | done |
| Open real entries on the device (store imported by hand, before clone exists) | device | later; clone may arrive first |

### M3 Face ID copies

| Item | Track | Status |
|---|---|---|
| Copy cache in the core: per-entry deadline from the key touch, sealed by a device keystore trait, wiped on expiry, clock moved back or reboot ends all copies, "Lock now" | face-id-copies | done |
| Simulated keystore and clock for Linux and the simulator | face-id-copies | done |
| Secure Enclave keystore in Swift (current face only, no passcode fallback) | screens | done; the owner opened entries with Face ID on the iPhone, 2026-11-23 |
| Early AutoFill skeleton: fill from a copy, shared index, staleness after a pull | autofill | superseded by the full extension in M7 |

### M4 Clone, setup, fetch

| Item | Track | Status |
|---|---|---|
| Card-signed SSH transport, pinned host keys | core | done |
| Integrate after fetch: fast-forward, replay local commits on top, keep both when one entry changed on both sides | sync | done |
| Setup flow as a core state machine: read key, address, check the host accepts the key before the PIN, clone, test decrypt | sync | done |
| GitHub host keys shipped in the app; first-connection confirm for other hosts | sync | done (core) |
| Setup screens | screens | done |
| First real clone of the owner's store from GitHub with the YubiKey | device | done 2026-11-23: setup, clone, download and decrypt worked on the iPhone |
| Pick the repository from a list after GitHub sign-in, instead of typing its address | screens | done; the owner signed in on the iPhone 2026-11-23 |

### M5 Write and push

| Item | Track | Status |
|---|---|---|
| Entry draft, password generator, encrypt to the recipient list, pass-style commit messages, delete, rename, move | core | done |
| Unpushed queue, undo, discard; push with fetch and replay on one login | sync | done |
| Add, edit, delete, rename screens | screens | done |
| A real push, read back by desktop `pass` | device | the owner sent a change from the iPhone 2026-11-23; reading it back with desktop `pass` not yet confirmed |

### M6 Silent pull

| Item | Status |
|---|---|
| GitHub sign-in (GitHub App, read-only on one repository) for background pull and the repository picker. The owner registered the app `paw-password-store` (client id `Iv23liyBePbnvQPCcGRz`, device sign-in enabled) on 2026-11-23 | done; signed in and checked for changes without the key on the iPhone 2026-11-23; token renewal after 8 hours not yet observed |
| Deploy key in the Secure Enclave for other hosts | later |
| Background refresh, "recipients changed" warning, replaced history | done; background refresh is scheduled and skips while the device is locked (the store is unreadable then) |

### M7 AutoFill, the rest

| Item | Status |
|---|---|
| AutoFill extension: list for the site, matching by registered domain and by name, fill with Face ID or the key, suggestions above the keyboard, confirm before filling another site, `url` cross-check, international names shown as `xn--`, store in the App Group | done; waiting on the owner's device checks. Not built: saving from the extension (needs iOS 26.2), one-time codes in the extension |

### M8 Settings and finish

| Item | Status |
|---|---|
| Settings, "Who can open these passwords", replace key, diagnostics export, remove store | done except replace key (design only): store, GitHub, AutoFill, who can open, Face ID switch, sites used, server keys with forget, key diagnostics with copy, licences, build stamp, remove store |
| Copy and UX polish passes | two passes merged (2026-11-23, 2026-11-24); further passes follow the owner's notes from the phone |
| App icon (paw built from discs) and wordmark rework | icon done (the paw); gold or pale toes still open |
| Licence notices for bundled libraries | done; `scripts/licences.py --check` runs in CI |

### M9 iPad

| Item | Status |
|---|---|
| Three-column layout, form sheets, keyboard shortcuts; Developer Mode on the iPad | done on the iPad simulator (its own CI part); device checks below |
| Device words on iPad ("iPhone", "Face ID" on a Touch ID iPad) | in progress, batch 5 |

### After the first version

Several recipients (the owner's backup key), TOTP, import from other managers, creating a store from scratch, NFC, keys without a YubiKey, public App Store release.

## Known gaps

Things that are easy to lose track of because nothing fails when they are missing.

| Gap | Why it matters |
|---|---|
| The Release build ran on the phone for the first time 2026-11-24 (run 45525329289) | the owner has not yet reported on it |
| Held-back changes are not re-encrypted automatically after recipients change | a manual action in the sync sheet is in progress, batch 5 |
| Three entries in the owner's store are encrypted to old keys | paw cannot open them; needs the owner |
| Swift unit tests cover the models (67); screens are still tested by clicking through the simulator | slow |
| Background-time wrapping, file protection, keyboard refusal, auto-close on lock | built, never observed on the device |

Merged 2026-11-24: plain repository list in the picker; per-process Face ID enrolment record; name matching with remembered site pairings (`site-pairings.json`, no screen to view or clear them yet); a lost PIN told apart from a missed touch; the key log.

Merged 2026-11-24, batch 3 (one build, run 45500441073, all green): Release device build without the simulated key, scanned in CI; legacy PIN-taking exports and the test screen removed; background refresh scheduled (skips while the iPhone is locked: the store is unreadable then); held-back re-encryption in the core (no UI yet); Settings: who can open, Face ID switch, sites used, server keys with forget, copy diagnostics, licences, build stamp; AutoFill: `url` cross-check, international names (always shown as `xn--`), username from the body; the paw icon; Sync, Gallery and Enclave tests on the shared helpers; iPad three-column layout with its own CI part.

Merged 2026-11-24, batch 4 (main `1231999`, installed on the iPhone as run 45525329289): fixes from two reviews (a locked background launch no longer offers to delete the store; server-key change warning; whole fingerprints; iPad folder selection; held-back flip-flop; Face ID switch ordering; clipboard expiry); a copy pass over Settings, sync, AutoFill and diagnostics; Swift unit tests (`PawTests`) in CI, and the seven UI tests they replace removed.

Not built: saving a password from the extension (needs iOS 26.2; minimum is 26.0); replace key (design only: both keys in turn, one touch per password); "iPad"/"Touch ID" wording on iPad. libgit2's bundled xdiff is LGPL: decide before any App Store listing.


| Check | Status |
|---|---|
| Settings › Key diagnostics › "Check the touch timeout", without touching the key; then copy the key log | waiting |
| First Release build: Settings › About says the commit and "Release"; open an entry, AutoFill fill, Face ID copy, send | installed 2026-11-24; waiting on the owner |
| "Read key" on the newest build, to see both slots' touch policies | waiting |
| First real clone | done 2026-11-23 |
| Auto-close after locking the phone and after 2 minutes away | waiting |
| GitHub sign-in still works the next day (token renewal) | waiting |
| After AutoFill installs: turn it on, fill with Face ID, fill with the key, suggestion bar, the confirm on another site, Face ID copy survives a fill | waiting on the build |
| A change sent from the phone reads back in desktop `pass` | waiting |
| Face ID fill in AutoFill after the per-process enrolment fix (2026-11-24): no "Face ID changed", and the copy survives | waiting |
| Name matches (entry `name` on `name.com`): first-time confirm, then no confirm, then a keyboard suggestion | waiting |
| First run of the Release build: Face ID, AutoFill, the key, the icon; Settings › About says the commit and "Release" | waiting |
| Background check row in the sync sheet after a few hours in the background | waiting |
| Who can open these passwords shows the three old-key entries | waiting |
| Settings › Key diagnostics › Key log shows the app's and AutoFill's lines; what the key answers after a touch timeout and to the PIN check after it | waiting |
| iPad: the key opens an entry over USB-C, and AutoFill fills with it | waiting |
| iPad: three columns in landscape, the right column first in portrait; Slide Over and a narrow window fall back to the iPhone layout without losing the open folder or entry | waiting |
| iPad: opening a second entry closes the first (nothing of it left in the right column); auto-close after locking and after 2 minutes away; the app-switcher cover | waiting |
| iPad with a keyboard: ⌘F, ⌘N, ⌘R, ⌘L, ⌘, and ⌘C (open entry only) listed when ⌘ is held; Esc cancels the key sheet; return sends the PIN; none of them opens anything without the key or Face ID | waiting |
| iPad: key sheet, editor, sync and Settings as centred form sheets; large type fills the screen | waiting |

## Open decisions for the owner

1. Ink-only buttons, or an indigo accent? Decided 2026-11-24: ink.
2. Paw icon or plain gold disc? Decided 2026-11-24: the paw.
3. Rent a Mac or pay for larger GitHub machines? Decided 2026-11-24: neither; the standard runners are fine for now.
4. Sheets: decided 2026-11-24: opaque. "Lock this password" shortened to "Lock": done. Still open: build a "forget server key" option?
