# paw roadmap

What is done, what is left, and what is being worked on. Kept current as work lands. The reasoning behind each milestone is in [design.md](design.md) section 7; the look is in [design-language.md](design-language.md); the rules are in [principles.md](principles.md).

Status words: **done** (on main, green on GitHub), **in progress** (a branch exists), **next** (unblocked, not started), **blocked** (waits on something named), **later**.

## How the work is run

- Building is done by subagents, each on its own branch in `.worktrees/<branch>`, one track per area so branches do not collide. Each branch is reviewed, built on GitHub, then merged.
- Rust core work is tested on Linux in minutes. Swift work compiles only on GitHub: a full run is about 20 minutes, a run narrowed to chosen tests is a few. Tracks are therefore cut so that as much as possible lands in the core.
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
| Raw card checks on the test screen | core + app | done |
| Exact touch policy per slot | core | done |
| Keep the simulated key and fixture out of device builds (cargo feature, device-library scan in CI) | foundations | next |
| App Group container and shared keychain group for app and extension, checked in the simulator | foundations | next |
| Key session in the core: one object per plugged-in key, PIN held for the session and forgotten after 15 minutes in the background, cancel, structured errors | key-session | done |
| Key task state machine (plug in, PIN, touch, working, done) driving the real key sheet | key-session, then screens | core done; Swift half next |
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
| Secure Enclave keystore in Swift (current face only, no passcode fallback) | screens | built; needs the device to verify |
| Early AutoFill skeleton: fill from a copy, shared index, staleness after a pull | autofill | blocked on App Group and copies |

### M4 Clone, setup, fetch

| Item | Track | Status |
|---|---|---|
| Card-signed SSH transport, pinned host keys | core | done |
| Integrate after fetch: fast-forward, replay local commits on top, keep both when one entry changed on both sides | sync | done |
| Setup flow as a core state machine: read key, address, check the host accepts the key before the PIN, clone, test decrypt | sync | done |
| GitHub host keys shipped in the app; first-connection confirm for other hosts | sync | done (core) |
| Setup screens | screens | in progress (branch `setup-screens`; end to end in the simulator against the simulated key and host) |
| First real clone of the owner's store from GitHub with the YubiKey | device | next: needs the owner, the phone and the key |

### M5 Write and push

| Item | Track | Status |
|---|---|---|
| Entry draft, password generator, encrypt to the recipient list, pass-style commit messages, delete, rename, move | core | done |
| Unpushed queue, undo, discard; push with fetch and replay on one login | sync | done |
| Add, edit, delete, rename screens | screens | later |
| A real push, read back by desktop `pass` | device | later |

### M6 Silent pull

| Item | Status |
|---|---|
| GitHub sign-in (GitHub App, read-only on one repository) for background pull; owner registers the app | later |
| Deploy key in the Secure Enclave for other hosts | later |
| Background refresh, "recipients changed" warning, replaced history | later |

### M7 AutoFill, the rest

| Item | Status |
|---|---|
| Key sheet inside the extension (proven reachable), suggestions above the keyboard, site matching, hand-off fallback | later |

### M8 Settings and finish

| Item | Status |
|---|---|
| Settings, "Who can open these passwords", replace key, diagnostics export, remove store | later |
| App icon (paw built from discs) and wordmark rework | later |
| Licence notices for bundled libraries | later |

### M9 iPad

| Item | Status |
|---|---|
| Three-column layout, form sheets, keyboard shortcuts; Developer Mode on the iPad | later |

### After the first version

Several recipients (the owner's backup key), TOTP, import from other managers, creating a store from scratch, NFC, keys without a YubiKey, public App Store release.

## Needs the device

| Check | Status |
|---|---|
| "Check the touch timeout (do not touch)" on the test screen | waiting |
| "Read key" on the newest build, to see both slots' touch policies | waiting |
| First real clone, once setup screens exist | next |

## Open decisions for the owner

1. Ink-only buttons, or an indigo accent? Draft answer: ink.
2. Paw icon or plain gold disc? Draft answer: paw.
3. Rent cloud Mac hours for visual polish, once real screens exist? Draft answer: try narrowed GitHub runs first.
