# paw design

> First draft, written by a design workflow on 2026-11-22 for the owner to edit. Settled since it was written:
> - Both the app and the AutoFill extension decrypt with the YubiKey over USB-C on an iPhone running iOS 27.2.1 (risk 8.2.1). The smart card entitlement is not needed on iOS and has been removed.
> - Background pull will offer GitHub sign-in (a GitHub App with read-only access to one repository) before the deploy key, which stays as the route for other hosts. Both are optional and come later.
> - A store with no remote is supported. Face ID copies are on by default. See "Decisions so far" in [principles.md](principles.md).

Scope: the design for paw v1 and the seams it leaves for later. Nothing was built, modified or run. Claims about the existing repo at `/home/toda/dev/pass-ios` were checked against the files and are noted inline.

Markers: **UNPROVEN** means the design depends on a fact nobody has verified. **OWNER** means a default only the owner can confirm (collected in 8.1).

---

## 1. Summary

paw is a thin SwiftUI app over one Rust crate, `paw-core`, which owns all state and runs every flow as a state machine. Swift renders snapshots and implements four platform traits: card reader, card connection, device keystore and clock.

Every use of the YubiKey (open an entry, clone, push, setup check) is a core object, `KeyTask`, started from a workflow's `KeyJob` on one driver, `KeyTaskDriver` (app/Shared/KeyTaskDriver.swift), and drawn by one sheet, `KeySheet`: plug in, PIN, touch. An entry opened with the key gets a copy sealed under a Secure Enclave key and stays available by Face ID for a fixed length of time measured from that touch (8 hours by default). Each copy has its own deadline.

Saving never needs the key or the network. Edits are single-path local commits, and the "not yet pushed" queue is the commits ahead of the server. Pull is silent with a read-only Secure Enclave deploy key. Local commits are replayed onto whatever the server has, never merged. When both sides changed one entry, both versions are kept automatically.

The AutoFill extension uses a narrow, read-only slice of the core. It fills from Face ID copies whether or not it can reach the key. When it cannot reach the key, it hands off to the app, which makes a copy the extension can open.

Every hardware dependency has a simulated twin in the core's `sim` module, behind the default `sim` cargo feature, so each flow runs on Linux and in the simulator; a Release device build leaves it out (`--no-default-features`, checked by `scripts/check-no-sim.sh`).

Things that changed relative to the owner's stated decisions, and need confirming:

- **Per-entry deadlines** instead of one shared window end (8.1.1).
- **A background limit on the held PIN** in addition to "once per plug-in" (8.1.2).
- **USB-C iPhones only** in v1. iOS 26 also runs on Lightning iPhones (11 to 14), which cannot take a YubiKey 5C NFC by cable.

---

## 2. User flows

### 2.1 Rules that hold across every flow

1. **The gold disc means "the key is needed" and nothing else.** It appears on a row, button or banner only when the next tap will ask for the YubiKey. It is derived from `Access::NeedsKey` or an operation's key requirement, never from "the user owns a YubiKey". No gesture opens the key sheet from a control that did not show the disc.
2. **One key sheet.** Every key operation is a `KeyJob` on a `KeyTaskDriver`, drawn by `KeySheet` with the same stages. Only the title changes. The touch timeout check in Key diagnostics shows the same stages, in the same words, inline.
3. **Saving never needs the key.** Encryption uses pinned public keys. Add, edit, delete and rename commit locally at once. Only push needs the key.
4. **Nothing waits on the network, and nothing presents itself.** Local actions finish first. Sync is reported in two places only (attention row, sync footer). No sheet opens without a tap.
5. **Errors say what happened, what is safe, and what to do**, in that order. They appear inline in the stage they belong to, never as alerts. Raw library text sits under "Details".
6. **Flow logic lives in the core.** Swift has no `if` about domain state that is not purely presentational.

### 2.2 Flow inventory

| Flow | Entry point | Happy path | Failure states |
|---|---|---|---|
| Setup | First launch | Plug in, address and key check, PIN and touch, download, touch to check, Face ID choice, AutoFill | Key not accepted by the server; clone failures (2.4); all key problems (2.11); killed mid-way (resumes) |
| Open with key | Tap a row with the disc | (Plug in), (PIN), touch, entry | All of 2.11 |
| Open with Face ID | Tap a row in the Face ID section | Face ID, entry | Face ID fails; biometric lockout; enrolment changed; copy expired; copy stale |
| Copy expiry | Time passes | Copy wiped; row gains the disc | Clock moved back or reboot (all copies end) |
| Lock now | Home menu, Settings, quick action | Wipe; toast | none |
| Browse and search | Home | Type; tap a result | No result |
| Copy | Row tap, long-press | Toast; clears after 45 s | none |
| Reveal and Large Type | Eye, "Show large" | Shown; auto-hide | Copy expires while shown; screen capture |
| Add | **+** | Form, Save, queued | Duplicate; invalid name; folder paw cannot encrypt for |
| Edit | Entry › Edit | Form, Save, queued | Copy expires mid-edit (draft kept in memory); read-only folder |
| Delete, rename, move | Entry menu, long-press | Confirm, queued | Different `.gpg-id` |
| Push | Attention row, "Send now" on the save toast | (PIN), touch, sent | Offline; key refused; server refused; server moved (automatic retry); unplugged |
| Silent pull | Foreground, refresh, background | Footer updates | Offline; device key refused; host key changed; history replaced; recipients changed |
| Both sides changed an entry | Automatic during pull or push | Both kept; informational row | none |
| AutoFill suggestion | Bar above the keyboard | Provider sheet, Face ID, filled | Needs key (key sheet or hand-off); domain mismatch |
| AutoFill list | Key icon | Pick, as above | Setup unfinished; nothing matches |
| Hand-off | Extension cannot reach the key | Open paw, touch, return, Face ID | Request older than 5 minutes (ignored); Face ID unavailable |
| Background updates setup | Home card, Settings | Copy, add, Check | Not accepted yet; key can write |
| Add a recipient's public key | Read-only folder, Settings › Key | Paste or import; fingerprint matches | No match with `.gpg-id` |
| Replace my key | Settings › Key | Plug in new key, touch to check | New key cannot open the store |
| Remove store | Settings | Confirm, Welcome | none |

### 2.3 Shared components

**The key sheet (`KeySheet`, drawn for a driver by `KeyTaskSheet`; its words in `app/Shared/Words/KeySheetWords.swift`).** A medium-height sheet (a centred form sheet on iPad). The title comes from the purpose: "Open github.com", "Download your store", "Send 2 changes", "Check your key", "Update your store", "Fill github.com". Cancel is always present.

| Stage (`KeySheetStage`) | Headline | Body | Controls |
|---|---|---|---|
| `PlugIn` | Plug in your YubiKey | USB-C, on the bottom of your iPhone. | Cancel |
| `Pin` | Enter your key's PIN | The PIN for the key, not your iPhone passcode. Asked once while the key stays plugged in. | Secure field `pin-field`, "Show" toggle, **Continue** (`pin-continue`, off until `min_len`, 6) |
| `KeyBusy` | One moment | Your key is finishing something else. | Cancel |
| `Touch` | Touch your YubiKey | Touch the gold contact on the key. It's blinking. | Drawing of the key with the contact highlighted and a 15-second ring (`touch-prompt`) |
| `ConfirmHost` | First time connecting to git.example.org | Fingerprint, key type, "Compare this with what your server's admin gave you" | **Trust this server**, Cancel |
| `Working` | From the purpose, e.g. "Downloading your store…" | "412 of 1,240" when known | Cancel |
| `Done` | none | none | Closes itself with a success haptic. No "Success" screen |

- The sheet opens at the first stage that still needs doing. Key in and PIN held means it opens at Touch.
- For an entry, the sheet names the entry before the card starts waiting, because one touch releases one named entry.
- `Pin` shows the tries left before any input whenever fewer than 3 remain: "2 tries left." or "1 try left before the key locks." With 1 left, Show turns on and the button reads **Use last try** from the start. Another computer may already have used tries.
- If paw dropped the held PIN because of the background limit (3.4), the `Pin` body adds: "paw forgot your PIN after 15 minutes away."
- The Touch drawing is the key, not a button. Tapping it shows "Touch the key itself, not the screen." The gold disc glyph alone is never used as the touch prompt.
- `Touch` is shown only once the card actor has actually sent the command that waits for a touch (3.13). Until then the stage is `KeyBusy`.
- For clone and push, `Working` ("Connecting to github.com…") comes before `Touch` as well as after, because the signature is requested on an open connection (3.5).
- If the relevant slot's touch policy is off, the Touch stage is skipped.
- Cancel during Touch closes the sheet at once. A late touch result is thrown away.
- The PIN field is a full keyboard with 6 to 127 ASCII characters and no content type. No PIN field sets a content type (it would invite a "Save Password?" prompt).

**The attention row.** One row at the top of Home shows the most important item, plus "and 2 more" when there are others. Tapping it opens that item, or a list when there are several. Priority: `HostKeyChanged`, `RemoteReplaced`, `RecipientsChanged`, `NeedsReencrypt`, `PushAuthFailed`, `PullAuthFailed`, `HandoffRequested`, `Unpushed`, `KeptBoth` (informational, dismissible). Offline is not an attention state.

**The sync footer.** One quiet line at the bottom of Home:

| State | Text |
|---|---|
| Device key set up, in sync | "Up to date · checked 2 min ago" |
| Just pulled | "Updated just now · 3 entries changed" |
| Offline | "Offline · last checked 09:12" |
| No device key | Gold disc + "Updates need your key · last checked Mon" (tappable: opens `KeySheet` "Update your store") |
| Sync stopped by the user or an attention item | "Sync paused · Review" (tappable: opens the attention item) |

### 2.4 First-run setup

Seven screens, one job each, Back on all but the first, resumable from a persisted `SetupStep`. Background updates are not in setup: adding a deploy key means signing in to the git host, whose password is in the store that is not yet usable. They are offered afterwards as a Home card.

**S0 Welcome.** Title "paw". Body: "Your pass store, opened with your YubiKey." Small print: "You'll need your YubiKey and the SSH address of your password store." Button: **Set up**. On an iPhone without USB-C: "paw needs an iPhone with USB-C for now." and no button. For the App Store, add "Try with demo data" (App Review has no YubiKey). That demo path needs a design of its own, because the simulated key must not exist in device builds (5.5).

**S1 Your key.** Headline "Plug in your YubiKey". It advances when a key appears. Then a card, "YubiKey · serial 12 345 678":
- "Decryption key · ends 2C11 41DE · needs a touch every time"
- "SSH key · ends 77B0 9F3A · needs a touch every time"

No PIN is asked; reading the key needs none. Button: **Continue**. Touch policy is per slot, so each line carries its own. Variants:
- Off: "no touch needed", with the warning "Your key does not ask for a touch for this. Anyone with this iPhone, your key and its PIN could read everything."
- Cached: "one touch covers about 15 seconds", with "During those seconds a second entry can be opened without another touch."
- No authentication key: see 2.11. Setup stops here.
- KDF enabled on the card: "This key uses a PIN setting (KDF) that paw doesn't support yet." Setup stops. No PIN try is spent (3.4).

paw still works with weaker touch policies. The policy is the user's.

**S2 Store address.** Headline "Where is your password store?" Field `repo-address`, placeholder `git@github.com:you/password-store.git`, Paste button. Button: **Connect**. Inline validation:
- `https://…`: "paw connects over SSH for now. Use the address that starts with git@ or ssh://."
- unparseable: "That doesn't look like a git address."

Connect runs a key check before any PIN or touch. paw connects, verifies the host key, and asks the server whether it would accept the card's SSH public key (an SSH public-key query, which carries no signature). Result line `key-check`:
- "github.com accepts this key." Setup moves to S3.
- "github.com doesn't know this key yet." Followed by "Your key's SSH key", one truncated line `ssh-ed25519 AAAA…`, and three ways to add it:
  - **Copy** and **Share…** (share sheet), plus **Show QR code** for a computer's camera.
  - "On your computer:" `gh ssh-key add` with the key inline, selectable.
  - **Open GitHub** (deep link to `https://github.com/settings/ssh/new`) when the host is github.com.
  - Then **Check again**.

A server that accepts the key but not the repository is caught at S3. **UNPROVEN:** that russh exposes the query step separately from signing.

**S3 Download.** `KeySheet` titled "Download your store": PIN, "Connecting…", touch, "Downloading your store…". An unknown host shows the `ConfirmHost` stage during S2's check, before any PIN. Bundled hosts (github.com, gitlab.com, codeberg.org, bitbucket.org, git.sr.ht) never show it. Success line: "Found 318 entries in 24 folders."

| Failure | Copy |
|---|---|
| Server refuses the key | "The server didn't accept this key." / "git@github.com answered, but this key's SSH key isn't allowed. Add it to your account, then try again." **Copy SSH key**, **Try again** |
| Repo missing | "No store at that address." / "The server says you/password-store doesn't exist, or this key can't see it." |
| Offline or DNS | "Can't reach github.com." / "Check your connection and try again." |
| Not a pass store | "That repository isn't a pass store." / "It has no .gpg-id file at the top." |
| Host key changed | "The server's identity changed." / "This can mean someone is intercepting the connection. Nothing was sent." Old and new fingerprints. "If the server's admin confirms the change, go to Settings › Store › Forget server key." **Stop** |
| Empty repo | "That store is empty." **Continue anyway** |

**S4 Check.** Headline "Check that your key opens it". Body: "paw will open one entry to be sure. It won't show or keep the password." The core picks the first entry whose recipient key ID matches the key's decryption key. `KeySheet` titled "Check your key" goes straight to Touch.
- Success: "It works. Your key opened 'github.com'." **Continue**. This step also pins the recipients (3.7).
- Wrong key, caught before the touch: "This key can't open this store."
- Decrypt failure: "paw downloaded your store but couldn't open 'github.com'." **Try another entry**, **Copy details**.
- The store has recipients paw has no public key for: "This store is also encrypted for 1 other key. You can open everything. To add or change entries, paw needs that key's public part." **Add it now** (2.12) / **Later**.

**S5 Face ID copies.** An explicit choice, never a silent default:

> **Keep opened passwords handy with Face ID?**
> After you open a password with your key, paw can keep a copy on this iPhone for 8 hours, locked with Face ID. During that time you can open it again, and fill it in Safari, without your key.
>
> For those hours, someone with this iPhone and your face could open those passwords without your key. Passwords you have not opened still need your key.
>
> Each copy is removed when its time is up, if Face ID is changed, or when you tap Lock now.

Picker: 15 minutes, 1 hour, 4 hours, **8 hours**, 12 hours. There is no "never". Buttons: **Keep for 8 hours** (the label follows the picker) and **Always ask for my key**. Under the second button: "With this off, AutoFill asks for your key every time." If Face ID or a passcode is not set up: "Face ID is off on this iPhone, so every entry will need your key. You can change this in Settings later."

**S6 AutoFill.** Headline "Fill passwords in Safari and apps". With the key still plugged in, paw first runs the reach probe (3.11): "Checking whether your key works inside Safari…". The body then states what the user will get:
- Probe says the key works in the extension: "Passwords you opened recently fill with Face ID. Others ask for your key's PIN and a touch, or you can open them in paw first."
- Probe says it does not, or could not tell: "Passwords you opened recently fill with Face ID. For others, paw will ask you to open paw and touch your key, then come back."

Button **Turn on AutoFill**. On return: "AutoFill is on." or "AutoFill is still off. You can turn it on in Settings › General › AutoFill & Passwords." with **Open Settings**. "Not now" is allowed. **UNPROVEN:** the system prompt API for enabling a credential provider on iOS 26; **Open Settings** is the fallback.

**S7 Done.** No screen; Home appears with the card below.

**Home card: "Turn on background updates".** Body: "This iPhone can check for changes on its own with a read-only key that never leaves it. It can't change your store." Opening it shows:
1. "Copy this iPhone's key" (**Copy**, **Share…**, **Show QR code**; suggested label "paw on iPhone, read-only").
2. "Add it to your repository as a read-only deploy key": **Open GitHub** (deep link to `https://github.com/<owner>/<repo>/settings/keys/new`) when the host is github.com; "On your computer:" the `gh repo deploy-key add` line; otherwise instructions.
3. **Check**.

Results: "Background updates are on." / "The server doesn't accept this iPhone's key yet." **Check again** / "This key can write to the repository. Add it as read-only." Dismissing the card: "Not now. paw will only update when you plug in your key and ask." The card can be reopened from Settings › Store.

UI tests (`SetupTests`):
- `testSetupHappyPath` asserts exactly two typed inputs (address, PIN) and a simulated decipher count of 1.
- `testSetupResumesAfterKill` kills at S4 and asserts no second download.
- `testKeyNotAcceptedStopsBeforePin`: with `-pawSimRemote authfail`, `key-check` shows the "doesn't know this key" line and `pin-field` never appears.
- One test per clone-failure row.
- `testWrongKeyStopsBeforeTouch` at S4.

**Outcome (branch `core-setup`).** The key steps of setup run as key tasks on the session (`Setup.read_key_from_session`, `download_with_key`, `test_decrypt_with_key`), so a whole setup asks for the PIN once; with the simulated key (SSH slot cached, decryption slot every use) it costs one VERIFY and two touches, one for the login and one for the test decrypt. `check_host` and `confirm_host` stay plain calls: they need no key. A task fails with the key's own `KeyError` for key problems; for anything else (host, repository, store) it ends `SetupFailed` with the `SetupError` for the S3 table. A cancelled or dropped download goes back to `NeedPin` with nothing left on disk, and the next try needs no PIN. When the test decrypt succeeds, setup pins the card's key, binds its `.gpg-id` line, accepts every `.gpg-id`, and records the key as the store's enrolled key (5.6). `Setup.write_readiness` says whether saving works with no further step, and otherwise which recipients paw needs keys for; setup is done either way. The binding no longer ties another recipient's line to the card when a line already names the card's key. The simulated host for `-pawSimRemote` is `SimRemote` (5.5).

**Outcome (branch `core-requests`, before the first real clone).** Additive; no exported function, signature or switched-over enum changed.
- Addresses (`setup/address.rs`): `owner/repo`, `github.com/owner/repo` and `https://github.com/owner/repo` (with or without `.git`, spaces and a trailing slash) become `git@github.com:owner/repo.git` with a notice. Refused: a password, token or user name in the address (never repeated back), `ext::` and other `<transport>::` forms, and anything starting with `-`. `read_store_address` gives the normalised address, user, host, port, notice and whether the host's keys ship with paw, for "paw will connect to github.com as git".
- Bundled host keys (`sync/hosts.rs`, `BUNDLED`): github.com and `ssh.github.com:443` (GitHub's keys, re-checked against `api.github.com/meta` on 2026-11-23), gitlab.com and bitbucket.org, each from the host's own documentation page cited beside it, with a test that every key has the fingerprint written beside it. codeberg.org and git.sr.ht publish fingerprints but not keys, so they stay first-use, unlike S3's list above. A bundled host's key cannot be forgotten (`Setup.forget_host_key`, `StoreSync.forget_host_key` refuse it).
- `Setup.forget_host_key()` forgets a confirmed host's changed key without leaving setup; `check_host` then shows the new key to confirm.
- `Setup.last_remote_message()` (`RemoteMessage`, `RemoteReason`) classifies the last host check or download: `RepositoryNotFound` (the host knows the key but says "Repository not found", reported as `SetupError::Failed`), `KeyNotAccepted`, `Unreachable`, `HostKeyChanged`, `EmptyRepository`, `NotAPassStore`, `Other`, with what the host printed. An empty repository or one with no `.gpg-id` now sends setup back to the address step (address kept, reason saved across a relaunch) instead of to the PIN step; S3's "Continue anyway" for an empty store is not built.
- Port 443: when github.com:22 cannot be connected to at all (TCP refused or timed out), `Setup.port_fallback()` offers `ssh.github.com:443`; only `use_port_fallback()` switches to it.
- `KeySession.pin_kdf()` / `pin_kdf_supported()`; setup refuses an unsupported PIN KDF at "Your key" (`UnusableKey`), so 2.11's KDF line is built for setup.
- The older per-call PIN paths (`open_entry`, `open_entry_keeping_copy`, `StoreSync.send`/`refresh`, `Setup.download`/`test_decrypt`) refuse a card other than the enrolled key before any VERIFY, as key tasks do. `PassStore` finds the state directory only when opened at `<state>/repo`.
- Clone and fetch progress is throttled to ten reports a second, ends at the total, and stops at a cancel; `KeyTask.transfer_progress()` and `Setup.transfer_progress()` add bytes (`TransferProgress`).
- `SimRemote.start_on(key, port)` and `SimRemote.serve(key, dir, port)` (kept repository, same address and host key after a relaunch), and `set_repository_not_found`.

### 2.5 Daily unlock

**Home**, top to bottom: attention row; "Open with Face ID" (each row shows "until 18:40"); "Recent, needs your key"; "Folders"; sync footer; bottom search with **+** beside it. Top-right menu: **Sync now**, **Settings**, and in a separate destructive section **Lock now**.

Home opens with no Face ID prompt by default. Names are plaintext in pass by design, and each secret is gated separately. "Require Face ID to open paw" (device passcode fallback allowed; a UI gate, not cryptography) is in Settings, off by default. **OWNER.**

| State | Home shows | Tapping an entry |
|---|---|---|
| Key absent, some copies live | Face ID section; other rows carry the disc | Covered: Face ID, then the entry. Otherwise `KeySheet` at Plug in |
| Key absent, no copies | No Face ID section; every row has the disc | `KeySheet` at Plug in |
| Key present, PIN not held | "Key connected" chip in the toolbar | `KeySheet` at PIN, then Touch |
| Key present, PIN held | Same chip | `KeySheet` at Touch only |
| Face ID copies off | No Face ID section, ever | Always the key |

**Copy rules.**
- A key decrypt seals a copy that lives for the chosen length from that touch. Opening an entry again with the key renews its copy. A Face ID open never extends anything.
- The key sheet's last frame and the entry footer say how long: "Open with Face ID until 02:35."
- Opening from a copy needs Face ID every time: one match releases one entry. Copying from an entry that is already open needs nothing further.
- No passcode fallback. Failure texts are separate:
  - No match: "Face ID didn't recognise you." **Try again**, **Use your key**.
  - Biometric lockout: "Face ID is turned off until you enter your iPhone passcode. Lock and unlock your iPhone, then try again." **Use your key**.
  - Face ID not enrolled: "Face ID isn't set up on this iPhone, so saved copies can't be opened." **Use your key**.
  - Passcode removed: "Your iPhone passcode was turned off, so saved copies were removed." **Use your key**.
- Expiry is enforced when a copy is read, not by a timer. A clock set backwards or a reboot ends every copy.
- If Face ID enrolment changes, the Secure Enclave invalidates the key. Home shows once: "Face ID changed on this iPhone, so saved copies were removed. Use your key."
- An entry changed by a pull loses its copy. An edit made while a copy is live re-seals the new version with the same deadline. A rename keeps the copy (copies are keyed by content, 3.8).
- A new entry saved with Face ID copies on gets a copy for the chosen length from the save. The user just typed that password on this phone, usually to fill it straight into a sign-up page. **OWNER** (8.1.10).
- Per-entry opt-out: an entry screen toggle "Always ask for my key" (stored locally by path). Such entries are never given a lasting copy. The entry menu also has "Forget Face ID copy".

**Expiry while the app is open.**
- The row moves from "Open with Face ID" to "Recent, needs your key".
- An open entry that was opened from its copy blanks its fields: "This entry's Face ID time ended at 18:40." with **Open with your key** (disc).
- An open editor keeps its draft in memory until closed, because saving needs no key.

**Leaving and coming back.** An open entry survives leaving the app for 2 minutes, so "copy username, switch, paste, come back, copy password" costs one touch or one Face ID. On return within 2 minutes the entry is masked but copyable. After 2 minutes away, a device lock, or Lock now, the entry screen shows "Closed while you were away." with the usual open button. A suspended process runs no code, so the plaintext is discarded when paw next runs, before anything is drawn (5.2 states this honestly).

**Lock now.** One tap, no confirmation and no undo: it is the panic control, and an accidental tap costs touches, not data. It wipes every copy, forgets the PIN, de-verifies the card if present, closes open entries, and clears the clipboard if paw filled it. Toast: "Locked. 6 entries now need your key." Also a Home Screen quick action.

UI tests (`UnlockTests`):
- `testOpenWithKeyThenFaceId`: decipher count stays 1.
- `testPinAskedOncePerPlugIn`: `pin-field` once and `touch-prompt` twice; still no `pin-field` after `sim-device-lock` / `sim-device-unlock`; `pin-field` again after `sim-unplug` / `sim-plug`; and again after `sim-advance-1h` in the background.
- `testCopyUserSwitchCopyPasswordOneTouch`: open, copy username, background 30 s, return, copy password; decipher count 1 and no Face ID prompt.
- `testCopyExpires`, `testCopyExpiresWhileEntryOpen`, `testLaterEntryOutlivesEarlier`, `testLockNow` (toast count, empty pasteboard), `testFaceIdFailureOffersKey`, `testFaceIdLockoutText`.

### 2.6 Browse and search

- **Folders** are grouped rows with a tinted icon chip, entry count and chevron. The colour is a stable hash of the top-level folder name into the Keyring 2 palette, with a local override in `config.json`.
- **Inside a folder:** subfolders first, then entries. An entry row shows its name, a secondary line (the path-derived username in `domain/user` layouts, otherwise nothing), and the disc unless the entry is covered.
- **Search** (placeholder "Search names and folders") is case- and diacritic-folded, and every query token must match. Ranking: name prefix, word prefix, name substring, folder match, then recency. Results show the folder as a secondary line.
- **Empty states:** "No entry or folder called 'xyz'." plus "Search looks at names only, never inside entries." and **Add 'xyz'**. Empty store: "Your store is empty." **Add an entry**.
- **Pull to refresh** runs a pull with the device key. With no device key it does not open a sheet (the gesture shows no disc). It scrolls the footer row "Updates need your key" into view and pulses it.
- **Long-press an entry:** Copy password (unlocks, copies, never shows the entry), Open, Rename or Move, Delete.
- **Folders in v1:** a folder is created by adding an entry with "New folder…" and disappears when its last entry is removed (git tracks no empty folders). Renaming or deleting a whole folder comes later; it is one change per entry and needs its own confirmation design.

### 2.7 Opened entry

Title: the entry name. Subtitle: the folder path.

| Row | Behaviour |
|---|---|
| **Password** | Dots. Tapping the row copies. Eye reveals; "Show large" opens Large Type |
| **Username** (`login`, `username` or `user`) | Tap to copy |
| **Website** (`url`) | Tap to copy; an arrow opens Safari |
| Other `key: value` lines | Tap to copy. Keys containing "pass", "secret", "pin" or "answer" start hidden |
| `otpauth://` line | "One-time code": the code in mono as "123 456" with a thin countdown ring; tap the copy button to copy it (same clipboard rules and toast as the password). Made from the open entry, no extra touch; refreshes each second. A line that can't make codes (`hotp`, a bad key) shows the reason. Never shown raw; edited only through "Setup key" |
| Notes (lines without a key) | Selectable text |
| Footer | "Open with Face ID until 18:40" or "Needs your key each time"; "Changed 3 Nov on laptop" from the git log |

Toolbar: **Edit**, and a menu with Rename or Move, "Always ask for my key", "Forget Face ID copy", "Copy path", Delete.

- **Copy.** Toast: "Password copied. Clears in 45 seconds." The item is local-only with a system expiry. On foreground, paw also clears it if the pasteboard is still paw's and the time has passed. **UNPROVEN:** that system expiry fires after the app is terminated.
- **Reveal** hides again after 30 seconds, on scene `inactive`, and on leaving the screen. While the screen is recorded or mirrored, reveal is disabled: "Hidden while the screen is being recorded." A screenshot taken while revealed re-masks and shows "That screenshot may contain a password. Delete it from Photos."
- **App switcher:** a blank cover replaces the whole window whenever the scene is inactive, because names are sensitive too.
- **Large Type:** full-screen, monospaced, three colours for digits, letters and symbols, position numbers under each character. Tap to close.
- **Memory.** Rust holds the only long-lived copy of the plaintext, for as long as the entry is open (including the 2-minute grace in 2.5). Swift receives short-lived copies for reveal, copy and fill; those cannot be wiped and are not claimed to be (3.3).
- **Read-only folder:** **Edit** is visible but disabled, with "paw can't save here yet: this folder is also encrypted for a key paw doesn't have." and **Add the missing key** (2.12).
- **Entry removed by a pull while open:** the fields stay, with a banner "This entry was deleted on laptop." **Edit** then saves it as a new entry.
- **Non-text entries:** "This entry isn't text. paw can't show or edit it." They are never rewritten.
- A masked field's accessibility value is "hidden", never the plaintext. This also keeps passwords out of XCUITest hierarchies and CI screenshots.

### 2.8 Add, edit, delete

**Add (`+`).** Form "New entry": Name (required), Folder (picker, default the current folder, "New folder…"), Username, Password (with Show and **Generate**), Website, Notes. **Save**.
- **Generate** is one button with fixed settings (20 characters from letters, digits and a conservative symbol set). Inventing a password on a phone keyboard is the alternative, so this is in v1. Options and policies come later. **OWNER** (8.1.6).
- File layout: password on line 1, then `login: …`, `url: …`, then notes. paw-written entries should carry `url` because it powers the AutoFill cross-check (5.3).
- Commit: "Add given password for Work/github.com to store." or "Add generated password for Work/github.com." (pass's own wordings).
- Inline validation: "Give it a name." / "Names can't contain /." / "There's already an entry called 'github.com' in Work."
- After saving: toast "Saved · Send now" (the second half carries the disc and appears only when the key is plugged in; otherwise "Saved. Waiting to be sent.").
- Folders paw cannot encrypt for are greyed out in the picker with "Needs a key paw doesn't have".

**Edit.** Needs the current plaintext, so it starts from an open entry. Same form, pre-filled; a "Text" toggle shows the raw file. Unknown lines, their order and `otpauth` lines survive byte for byte (a core property test). Commit: "Edit password for Work/github.com using paw." Discard: "Discard your changes?" with **Discard** / **Keep editing**. No plaintext draft is ever persisted. An edit interrupted by termination is lost by design.

**Delete, rename, move.** These need no key.
- Delete confirms: "Delete 'github.com'?" / "It stays in your store's git history." Commit: "Remove Work/github.com from store." Its copy is wiped.
- Rename or move is allowed only when source and destination share a `.gpg-id`; otherwise "Moving between folders with different keys comes later." Commit: "Rename Work/github.com to Work/github.com-old."

**The "not yet pushed" queue.**
- Attention row: gold disc plus "Send 2 changes".
- Screen "Not yet pushed", newest first: "Edited github.com · 14:02", "Added Bank/monzo · yesterday", "Deleted old-wifi · Mon".
- Primary button **Send with your key** (disc). The newest row swipes to **Undo**. At the bottom: "Discard all changes…" with "Discard 2 changes? The store goes back to what's on the server."
- Nothing opens by itself after a save or when the key is plugged in. Five saves in a row are five toasts and one push with one touch.

UI tests (`EditTests`): `testAddWithoutKey`, `testEditWithLiveCopyQueues`, `testFiveSavesOnePushOneTouch`, `testPushDrainsQueue` (simulated remote head equals local head), `testUndoNewest`, `testDuplicateName`, `testDeleteConfirm`, `testSendNowFromInsideEditorSheet` (the key sheet presents over a presented sheet, 3.12).

**Outcome (branch `write-path`).** Built in `core/src/write/` (`draft`, `ops`, `keyring`, `cert`), `core/src/generate.rs` and `core/src/ffi/write.rs` (`StoreWriter`, `EntryDraft`), tested on Linux against GnuPG and git. Each change is one commit of one entry in pass's wording, always authored "paw <paw@paw.invalid>" (there is no per-store git identity); all of them list, undo and replay with the existing queue and integrate. Differences from the above:
- Moving to a folder with other recipients is built, not "later": it re-encrypts, as `pass mv` does, and so is an edit of the open entry whose folder changed; there is no move or rename without the entry open. It is one commit worded "Rename …", which the replay now accepts with a changed blob.
- An edit that also renames is two commits, "Rename …" then "Edit …".
- An edit, a rename or a move re-seals the entry's Face ID copy for the time it had left. A new entry gets a copy for the length passed in.
- Recipient keys are pinned in a small text file `recipients` in the state directory, not `keyring.json`. A `.gpg-id` line is satisfied by an explicit binding (setup binds the one line the card's subkey answers for after the test decrypt), or else by exactly one pinned key it names by full fingerprint or long key ID (of the encryption key paw chose or of the certificate's verified primary key) or exact e-mail address. Unlike 3.7's "any subkey fingerprint", a line naming any other subkey (older, revoked, expired, unbound) is refused with the reason and needs an explicit binding. Short IDs and names need a binding. paw always says when recipients change: setup calls `accept_all_recipients_at_setup`, which records every `.gpg-id` (path and contents) in the state directory; nothing is accepted implicitly after that. A folder whose governing `.gpg-id` is now another file (one added deeper or removed by a pull) or has other contents refuses saving with `RecipientsChanged` (old and new lines) until `accept_recipients` for that folder; before setup has recorded anything, saving is refused everywhere.
- Names are checked for collisions ignoring letter case and Unicode normalisation, as APFS does.
- The generator takes a policy; the button uses the fixed default (20 characters, all four classes, symbols `!#%+-.=?@_`, look-alikes `0O1lIo` left out, about 120.7 bits). There is no word mode: no word list with a clear licence is embedded yet.

### 2.9 Sync

**Silent pull.** Runs on foreground, pull to refresh and system background refresh, with the device key. No UI except the footer. With no local changes it fast-forwards. With local changes the core replays them onto the server's tip, without the key. paw's commits are unsigned. **OWNER.**

**Push.** `KeySheet` titled "Send 2 changes": PIN if needed, "Connecting…", touch, "Sending…", done. One push is one SSH connection and one touch. The fetch, any replay and the send all run on that connection, including the retries when the server moved (at most 3 loops). Before any PIN or touch the core checks reachability and whether the server accepts the key (3.5). **UNPROVEN:** that the git hosts allow an upload-pack and a receive-pack session channel on one authenticated connection (standard SSH; OpenSSH connection sharing works against GitHub). Fallback: a second connection and a second touch, only when the server moved.

**Offline.** Everything local works. Footer: "Offline · last checked 09:12". A push attempt shows, before any PIN or touch: "You're offline. Your changes are saved on this iPhone and will wait."

**Both sides changed the same entry.** Each entry is one opaque encrypted file, so the only conflict is the same path changed in both places. Both versions are ciphertext, so the user cannot choose between them blind. paw therefore never asks; it keeps both.
- Edited on both sides: the server's version keeps the name. The iPhone's version is saved beside it as "github.com (iPhone copy)" (a number is appended if that exists). Row: "Two versions of github.com were kept." The detail lists both with **Open** on each (disc where needed) so the user can compare and delete one.
- Edited here, deleted there: the iPhone's version is kept under its own name. Row: "github.com was deleted on laptop but changed here. Your version was kept."
- Deleted here, edited there: the server's version stays and the local delete is dropped. Row: "github.com was changed on laptop, so it wasn't deleted."
- Deleted on both sides: nothing to do, no row.

These rows are informational and dismissible. Sync is never blocked and nothing is lost. "Changed on laptop" comes from the commit author name.

**History replaced** (the server's head is not a descendant of the last one paw saw): sync stops. "The store on the server was replaced." / "Its history no longer matches this iPhone's. Nothing here has been changed." Buttons:
- **Use the server's store and keep my 2 changes** (default when a queue exists; the changes are replayed onto the new history with the rules above). With no queue: **Use the server's store**.
- **Use the server's store and discard my changes** (confirm "Your 2 unpushed changes will be lost.").
- **Not now**. The footer then reads "Sync paused · Review" and the attention row keeps the item, with any other items behind "and 2 more".

**Recipients changed by a pull.** Attention row: "Who can open work/ changed." Detail: "Changed on laptop, 3 Nov. Before: 1 key (yours). Now: 2 keys (yours, and one ending 9A1C 03F2)." with fingerprints under Details, and **Accept** / **Not now**. Until accepted, paw does not write in that subtree (5.3). After **Accept**, if paw lacks a public key for a new recipient, the next screen is "Add the missing key" (2.12); until that is done the folder stays read-only and says so.

**A queued change is encrypted for the old recipients** (`NeedsReencrypt`). If a pull changes the governing `.gpg-id` of a path with a queued change, that change is taken out of the queue and parked (3.6). Sync sheet row: "1 change wasn't sent", "Who can open the folder changed after you saved." **Review** opens the held-back screen. Once the new list is accepted, **Re-encrypt 1 change** (disc) sits in the sync sheet's bottom actions (`sync-reencrypt`) and on the held-back screen (5.3, `held-back`).

**Auth failures.**

| Case | Where | Copy |
|---|---|---|
| Device key refused | Attention row | "Background updates stopped." / "The server no longer accepts this iPhone's key." **Fix** opens the background-updates screen |
| YubiKey refused on push | Key sheet, before PIN or touch | "The server didn't accept this key." / "Your changes are still waiting on this iPhone." **Copy SSH key**, **Try again** |
| Push refused by a server rule | Key sheet | "The server refused the changes." / server message under Details |
| Host key changed | Attention row or sheet | "The server's identity changed." / "paw stopped syncing to be safe. If the server's admin confirms the change, go to Settings › Store › Forget server key." Old and new fingerprints |

The only way past a host key change is Settings › Store › "Forget server key", a separate deliberate act. It works for bundled hosts too.

UI tests (`SyncTests`):
- Drive `-pawSimRemote` through `ok`, `offline`, `diverged`, `conflict`, `replaced`, `authfail`, `hostkeychanged`, `recipientschanged`.
- `diverged` asserts exactly one touch and one connection.
- `conflict` asserts the simulated remote's file list for each of the four cases.
- `testNoDeviceKeyFooterAndRefresh`: footer shows the disc; pull to refresh opens no sheet.
- Replay, keep-both and parking get most of their coverage on Linux against two temporary repositories.

### 2.10 AutoFill

**What the system is told.** `ASCredentialIdentityStore` gets, per identity:
- a service: the domain from a file or folder name that parses as a domain, or a domain the user associated with the entry (below);
- a user string: the file name in `domain/user` layouts, otherwise the entry name (a blank row looks broken).

Nothing learned from a decrypted entry is stored (5.6, rule 6), with one exception the owner turns on: which entries have a one-time code (below).

**Matching** is exact on registrable domain, using a compiled-in public suffix list, after IDNA normalisation. There is no substring or fuzzy match. Names containing bidirectional controls or mixed-script confusables are never offered automatically. Exact host ranks above registrable domain; covered entries rank first.

Known limit: hosts that serve many tenants but that the public suffix list does not split (`sharepoint.com`, `atlassian.net`, `zendesk.com`, `wordpress.com`, `myshopify.com`) count as one registrable domain, so an entry for one tenant is offered on every other tenant's site. IPv4 addresses in any form (`127.1`, `0x7f.0.0.1`, `10.0.0.0x1`) have no registrable domain and match only the same text.

**Name matches** (built 2026-11-24, replacing the planned associations). Most real stores have entries called `bankofamerica` or `alaskaair/alice` that never parse as a domain. Such an entry matches a site when its own name, or its direct parent folder unless that is a category word (`bank`, `web`, `mail`, `work`…), squeezed of case, spaces, `-` and `_`, equals the first label of the site's registrable domain (`bankofamerica` for `www.bankofamerica.com`; `alice`, not `github`, for `alice.github.io`). Neither counts when a host comes earlier in the path. Whole parts only, at least 3 ASCII letters or digits; never an IP address, a dotless host or a `xn--` label (core/src/store/service.rs).

A name fits lookalikes (`bankofamerica.co`), so paw keeps, per entry, the registrable domains it has filled on after the person confirmed (`site-pairings.json` in the state directory: not in the repository, not synced, removed with the store, bounded to 300 entries of 4 domains). In the list:
- Paired with this domain: under "For <site>", fills without asking, published above the keyboard for that domain only.
- Not yet paired: under "Might be for this site", below. Picking it asks once, in plain colours: "Use bankofamerica on bankofamerica.com?" / "First time on this site. Check the address is right." The pairing is kept only after the fill succeeds; then it fills without asking.
- Paired only with other domains of that name (a lookalike): not listed; found by search, it asks in red: "Use bankofamerica on bankofamerica.co?" / "Used before on bankofamerica.com." **Fill** fills once and never pairs (the core refuses), so it warns every time. Trusting a second domain of one name means clearing the entry's pairings, which has no screen yet.
- Another site's password, found by search: "Fill GitHub's password here?" / "This password isn't saved for this site.", with a warning mark. Never paired.

**Confirm screen.** One layout for every case: headline, one line, the site's address in full, then **Cancel** (primary) and **Fill**. Hosts are always shown in the `xn--` form iOS gives, so a lookalike spelling shows. Once opened, an entry whose `url:` line names another registrable domain asks the same way, in red: "Saved for github.com, not example.net", plus "Fill needs your key again." when there is no Face ID copy to fill from.

Pairings involve no decrypted content, so rule 6 holds. The core can list and clear them (`SitePairings`); Settings › Sites used shows them.

**Tapping a suggestion.** `provideCredentialWithoutUserInteraction` always returns `userInteractionRequired`, so the provider always presents a view: "Fill github.com" behind the Face ID prompt, or the key sheet with that purpose ("Fill fastmail on example.com" when the name doesn't say the site).

| Entry state | What happens |
|---|---|
| Covered | Face ID, then filled |
| Needs the key | `KeySheet` in the provider: it asks for the PIN on every key use (the app's held PIN is never shared with it), then "Then it fills with Face ID for 8 hours." |

**Provider sheet** (key icon or "Passwords…"). Title "paw", bottom search.
- "For github.com": the site's entries; "Might be for this site": name matches, below.
- Nothing matches: "No password for this site." over the address. No site (an app): "paw can't tell which site this is."
- Search: the site's sections first, then "Other passwords"; nothing found: "No matches."
- An entry with no password: "<name> has no password" / "Pick another entry."

**One-time codes** (built 2026-11-25; iOS 18 `ProvidesOneTimeCodes`). The key icon in a code field opens the same sheet for a code: same matching, the same confirm rules ("Fill github.com's code here?" / "This code isn't saved for this site."), the same lookalike refusal, Face ID from the copy or the key sheet ("Fill code for github.com"). The code is made from the entry at that moment and handed to iOS; neither it nor the secret is kept. With under 3 seconds left, the next code is filled instead.

Which entries have a code is learned only by opening them, so suggesting codes needs a list kept outside a sealed copy. It is off until the owner turns it on: Settings › AutoFill, "Suggest one-time codes", footer "Keeps a list of which passwords have a code." The switch is a marker file beside the list (`one-time-codes-on`), read by the app and the extension alike (core `OneTimeCodeEntries.enabled`).
- Off (the default): nothing is written, no `ASOneTimeCodeCredentialIdentity` is published, and a code request lists nothing for the site: "No codes for this site. Search for one." Search, Face ID and the key fill a code as usual. Turning it off deletes the list and republishes without code identities.
- On: each time an entry is opened, filled or saved on this iPhone, paw keeps a yes per entry path that has a working code (`one-time-codes.json` in the state directory: not synced, removed with the store). The extension writes only after the fill is handed over, and only when the answer differs from the list. The code list shows the site's entries with a yes; the app publishes an `ASOneTimeCodeCredentialIdentity` next to each password identity of such an entry, on each launch and return to the foreground, so marks the extension added reach iOS then.

Marks follow renames, moves and deletes made in paw on this iPhone. A pull does not touch them: an entry deleted or renamed elsewhere drops out of the list (only paths still in the store are read), and its new name has no mark until it is opened here. An entry whose code was removed elsewhere stays listed and suggested; picking it says "<name> has no one-time code" and the mark goes.

**Not built** (roadmap M7): the choice screen and the hand-off below, for when the extension can't reach the key. Whether the in-extension key path is worth keeping is decided after the device spike.

**Hand-off screen** (not built), titled "github.com needs your key":
1. "Open paw."
2. "Touch your key when it asks."
3. "Go back to Safari and tap the password field again."

Button **Cancel** removes the request. So does any other dismissal of the provider. The extension writes `HandoffRequest { store, path, service, at }` (path-derived data only) to the shared container. When paw next comes to the foreground within 5 minutes, Home shows a banner in the attention row: "Safari asked for github.com" with **Open with your key** (disc). Nothing is presented automatically. On success: "Ready. Go back to Safari and tap the password field again." **UNPROVEN:** whether a credential provider may open its containing app. Assumed not, so step 1 is an instruction, not a button.

**What the hand-off leaves behind.**
- Face ID copies on, entry not opted out: an ordinary copy with the ordinary deadline.
- Face ID copies off, or the entry is marked "Always ask for my key": a **single-use copy**, sealed the same way, that expires after 2 minutes and is deleted on its first open. This is in v1, because without it a user who chose "Always ask for my key" has an AutoFill that can never fill when the extension cannot reach the key. The success text is then: "Ready for 2 minutes. Go back to Safari and tap the password field again."
- Face ID not set up at all: no copy can be made. The hand-off screen is replaced by "AutoFill needs Face ID on this iPhone, or a key that works inside Safari. Copy the password from paw instead."

**Other states.**

| State | Copy |
|---|---|
| Setup not finished | "Set up paw first" / "Open paw, then come back here." |
| PIN blocked | Same text as the app |
| Entry over 1 MiB | "Open this one in paw." |

UI tests (`AutoFillTests`): the provider's root view lives in `Shared` and is hosted in the app with `-pawHostProvider https://github.com`. Assert:
- section contents and order;
- a copy fill with `sim-faceid-match` (a test-only label shows the returned username, never the password);
- a key fill that then appears in the Face ID section;
- exact hand-off text with reach forced to `Blocked`, the Home banner, and the single-use copy opening once and not twice;
- the association offer and its effect on the next match.

The real extension process, the system Face ID prompt and the suggestion bar are a manual device checklist.

### 2.11 Hardware failures

All inline in `KeySheet` (`key-error`) unless noted.

| Failure | Detected | Headline | Body and actions |
|---|---|---|---|
| Wrong PIN, 2 left | After verify | "Wrong PIN. 2 tries left." | Field clears, stays focused |
| Wrong PIN, 1 left | After verify, or on reading the key | "Wrong PIN. 1 try left before the key locks." (or "1 try left before the key locks." when nothing was typed yet) | Show turns on; button becomes **Use last try**; "If you're unsure, stop and check your PIN first." |
| PIN blocked | Third wrong PIN, or reading the key | "The PIN is locked." | "Three wrong PINs lock the key. Unlock it on a computer with your admin PIN, for example with `ykman openpgp access unblock-pin`. Entries open with Face ID still work until their time is up." **Done**. Verify the command before shipping |
| Touch timed out | Key returns after about 15 s | "The key wasn't touched in time." | **Try again**. Stays on Touch; no PIN again |
| Unplugged mid-operation | Transport error or reader removal | "The key was unplugged." | Back to Plug in: "Plug it back in to continue." PIN asked again. During push add "Your changes are still waiting."; during first download add "The download will start again." |
| Wrong key | Before the PIN | "This key can't open this store." | "The store is encrypted for a key ending 2C11 41DE. This key ends 77B0 9F3A." **Try another key**. No PIN try spent |
| No decryption key | Reading the key | "This key has no decryption key." | "paw needs a YubiKey with an OpenPGP decryption key. This one (serial 12 345 678) has none." |
| No SSH key | Setup S1, or push | "This key has no SSH key." | "paw uses the key's authentication key to reach your store. Add an authentication subkey to this key on a computer (`gpg --edit-key`, then `addcardkey`), then try again. This key can still open entries." Setup blocks; daily use allows opening |
| KDF enabled | Reading the key | "paw doesn't support this key's PIN setting yet." | "The key has KDF turned on. No PIN try was used." |
| No OpenPGP applet | Reading the key | "paw can't find OpenPGP on this key." | "It may be turned off for USB, or this may not be a YubiKey." |
| Key busy | Session not granted in 10 s | "Another app is using the key." | "Unplug it and plug it back in." |
| No smart card support | Opening the connection | "This iPhone can't talk to USB keys." | Setup only |
| Entry not readable | Parsing | "'github.com' isn't a readable entry." | "The file may be damaged. Other entries are not affected." |
| Hidden recipient, decrypt fails | After touch | "This key couldn't open 'github.com'." | The wrong-key pre-check cannot run when key IDs are hidden (`throw-keyids`) |

A second YubiKey that holds the same subkeys is not a wrong key. Keys are matched on the decryption fingerprint; the serial is a label (3.4).

The spike's wrong-PIN strings in `app/Shared/KeyConnection.swift` (`describe`) are kept. The tests in `app/PawUITests/KeyTests.swift` assert on the spike's `latest-result` element and must be rewritten against the sheet.

`KeyFailureTests` assertions that matter:
- a wrong PIN never shows `touch-prompt`;
- with `-pawSimKey tries1` the sheet shows "1 try left" and **Use last try** before anything is typed;
- at one try left after a wrong PIN the button reads "Use last try";
- after a touch timeout, **Try again** succeeds without `pin-field`, and the try counter is unchanged;
- unplug during Touch returns to Plug in and the next open asks the PIN;
- wrong key, no decryption key and KDF never show `pin-field`, and the try counter stays at 3;
- cancel during Touch followed at once by opening another entry shows `KeyBusy`, not `touch-prompt`, until the first command has ended;
- while blocked, a covered entry still opens with Face ID.

### 2.12 Settings

| Section | Rows |
|---|---|
| **Key** | "YubiKey · serial 12 345 678" (or "Not plugged in"); fingerprints; "PIN tries left: 3"; per-slot touch policy with warnings; **Copy SSH key**; "Other keys that can open this store" (imported public keys, with **Add a key…**); **Replace my key…**; "Key diagnostics" (what the key said when plugged in, the touch timeout check, the key log) |
| **Face ID copies** | On/off; Length; "6 entries · the last until 18:40"; **Lock now**. Turning it off wipes copies at once |
| **Store** | Address; sync status; **Sync now**; Background updates (On, Off, "Stopped: key refused"); "Name on changes" (default "paw on iPhone"); "Forget server key" |
| **AutoFill** | Status with **Open Settings**; "Key inside AutoFill: Works / Not available on this iOS version / Not checked yet" with **Check now** (needs the key plugged in); Associations |
| **Clipboard** | "Clear after" (30, 45, 90 seconds; no "never") |
| **Privacy** | "Require Face ID to open paw" (off), with the caption "Turning this on removes paw's suggestions from the keyboard bar. The key icon still works."; "Clear recents" |
| **About** | Version; **Copy diagnostics** (never names or secrets; previewed before copying); Licences |
| **Remove** | "Remove store from this iPhone…" |

**Add a key** (a recipient's public key). "Paste the public key, or choose a file." paw parses the armoured certificate, shows its user ID and fingerprint, and matches it against the `.gpg-id` lines it could not resolve. Match: "This is the missing key for work/. paw can now save there." No match: "No folder in your store is encrypted for this key." Nothing is fetched from a keyserver.

**Replace my key** (lost or new YubiKey). "Plug in the new key." paw reads it and runs the S4 check against the store. If the new key can open the store (the store was already re-encrypted for it elsewhere, or it holds the same subkeys), paw pins it, wipes all Face ID copies, and reminds the user to remove the old key's SSH key from the git host. If it cannot: "This key can't open this store yet. Re-encrypt the store for it on a computer first." Re-encrypting on the phone costs one touch per entry and is not in v1.

**Changing the store address.** The address row is editable. On save, paw fetches from the new address with the usual key sheet and accepts it only if its history contains the last commit paw saw: "Same store, new address. Updated." Otherwise: "That's a different store. Remove this store first to use it."

**Moving to a new iPhone.** Nothing transfers; device backups exclude paw's data. Setup runs again on the new phone. On the old phone, Remove store.

Remove confirmation: "This deletes the downloaded store, Face ID copies and this iPhone's key. Your store on the server is not touched." With a queue, add "2 unpushed changes will be lost." Afterwards paw reminds the user to remove the deploy key on the host.

### 2.13 iPad differences

- Three columns: folders with the attention row and footer; entries, with search on top; the open entry.
- `KeySheet` is a centred form sheet. Plug-in body: "USB-C, on the edge of your iPad."
- Add and Edit are form sheets over the split view.
- Expiry blanks only the detail column.
- Shortcuts: ⌘F search, ⌘N new entry, ⌘C copy password of the selected entry, ⌘L Lock now.
- Every "Face ID" string comes from one function that returns the device's biometric name (Touch ID iPads).
- CI's UI test matrix adds `iPad` when the split view lands. The identifiers and tests are the same.

---

## 3. Architecture

### 3.1 Crates and modules

One crate. The simulated hardware is the `sim/` module behind the default `sim` cargo feature; a Release device build is made with `--no-default-features`, and `scripts/check-no-sim.sh` proves the simulated key's constant secret is not in it (5.5). The tree below is the plan the crate grew from; `core/src` is the truth.

```
core/            paw-core (device and simulator)
  src/
    error.rs        PawError and sub-errors; present() -> ErrorPresentation
    platform.rs     foreign traits: CardReader, CardConnection, DeviceKeystore, Clock
    model/
      path.rs       EntryPath, FolderPath (validated, NFC-normalised, no "..", no leading ".")
      entry.rs      EntryDoc: lossless parse/serialise
      generate.rs   generate(): fixed-policy password
    store/
      layout.rs     container paths; file-protection intent per path
      tree.rs       StoreTree: walk the working tree
      recipients.rs .gpg-id resolution, pins, Keyring, Resolution, certificate import
      index.rs      NameIndex: search and service matching; index.bin; associations
      recents.rs    per-process append logs, merged by the app
    crypto/
      openpgp.rs    the only file importing `pgp`: recipient key IDs, decrypt via a card slot, SEIPD v1 encrypt, certificate parsing
    card/
      trace.rs      ApduRecord { ins, sw, elapsed }; the recording CardTransaction wrapper
      status.rs     trace + phase -> CardError
      openpgp.rs    CardSession: info, verify, verify-status, de-verify, decipher, internal_authenticate, public keys, KDF check
      actor.rs      CardActor: the one thread that touches the connection
    unlock/
      session.rs    KeySession: plug-in session, zeroising PIN
      attempt.rs    UnlockAttempt: the card half of a key operation
      task.rs       KeyTask (now key/task.rs): what the sheet's driver follows; Cancel marks it cancelled at once
    window/
      seal.rs       sealed-copy format (AEAD with AAD)
      cache.rs      CopyCache: add, open, check, sweep, end_all
    sync/
      repo.rs       git library wrapper: open, tip, tree edit, commit, checkout, log
      queue.rs      PendingChange derived from commits ahead of origin
      replay.rs     replay onto a new tip; keep-both; re-encrypt parking
      connector.rs  Connector trait
      ssh.rs        russh Connector; SshSigner impls; known hosts; key query
      push.rs       send-pack (only if gix; see 3.10)
      op.rs         SyncOp: one network operation with progress and cancel
      engine.rs     SyncEngine state machine
    autofill.rs     plan(row, reach) -> AutofillPlan
    setup.rs        SetupFlow, SetupStep
    ffi/            PawCore, Store, OpenEntry, EntryDraft, KeyTask, SyncHandle, AutofillCore
  src/sim/        feature `sim` (default on): SimulatedKey, SimKeystore, SimClock, SimRemote, the fixture store
```

Import rules:
- `model` and `crypto` do no I/O.
- `store` reads files but never decrypts.
- `sync` never sees plaintext.
- `window` never sees the card.
- `autofill.rs` and everything `AutofillCore` reaches never name `sync`.
- Nothing in `paw-core` can name `paw-sim`.

**The split is real build work**, budgeted in M1. Today `scripts/build-core.sh` copies one `libpaw_core.a` per target and generates one binding from a host build, and `scripts/make-xcframework.sh` gives both slices the same headers. The split needs:
- a different library per slice (device: `paw-core`; simulator: the umbrella);
- two binding modules (`paw_core.swift`, `paw_sim.swift`) with per-slice headers and module maps;
- UniFFI trait objects that cross crates (`SimulatedKey` implements `paw-core`'s `CardConnection`).

### 3.2 Platform seams

Swift implements exactly these four foreign traits. Pasteboard, `ASCredentialIdentityStore`, Darwin notifications and background-task scheduling stay in Swift as thin OS calls fed with data the core returns.

```rust
pub trait CardReader: Send + Sync {          // was KeyConnection.open
    fn connect(&self) -> Result<Arc<dyn CardConnection>, ConnectionError>; // NotPresent | Busy | Failed
}
pub trait CardConnection: Send + Sync {      // exists today; loses touch_needed
    fn transmit(&self, apdu: Vec<u8>) -> Result<Vec<u8>, ConnectionError>;
    fn abort(&self);                          // makes a blocked transmit return promptly; idempotent
}
pub trait DeviceKeystore: Send + Sync {      // Secure Enclave on device
    fn create_key(&self, tag: String, policy: KeyPolicy) -> Result<Vec<u8>, KeystoreError>;
    fn delete_key(&self, tag: String) -> Result<(), KeystoreError>;
    fn wrap(&self, tag: String, data: Vec<u8>) -> Result<Vec<u8>, KeystoreError>;     // public-key op, no prompt
    fn unwrap(&self, tag: String, wrapped: Vec<u8>, reason: String) -> Result<Vec<u8>, KeystoreError>; // Face ID; blocks
    fn sign(&self, tag: String, data: Vec<u8>) -> Result<Vec<u8>, KeystoreError>;     // ECDSA P-256, deploy key
}
pub enum KeyPolicy { BiometryCurrentSet, AfterFirstUnlockNoPrompt }
pub trait Clock: Send + Sync {
    fn wall_ms(&self) -> i64;
    fn continuous_ms(&self) -> i64;   // counts while asleep, resets on reboot
    fn boot_id(&self) -> String;      // kern.bootsessionuuid on device
}
```

`touch_needed` leaves `CardConnection`: the core's own APDU recorder knows when it sends a command that waits for a touch (3.14).

| Seam | Device | Simulator and Linux |
|---|---|---|
| Card | `KeyReader` over `TKSmartCardSlotManager`, `UsbCardConnection`; later an NFC reader | `SimulatedKey` (Rust), held once per launch by `SimulatedKeyHolder` |
| Keystore and biometrics | `EnclaveKeystore` | `SimKeystore`: software P-256; `unwrap` blocks until `SimWorld::face_id(match)` |
| Clock | system clocks | `SimClock`, with `SimWorld::advance(seconds)` and `SimWorld::reboot()` |
| Network | `SshConnector` (russh) | `LocalConnector` to a bare repo, wrapped by `SimRemote` for faults; it demands and verifies a signature from the configured `SshSigner` and answers key queries |

Notes:
- `KeyReader` follows presence by KVO on `TKSmartCardSlotManager.slotNames` and reads a key that comes in into the `KeyPort`'s session; in the simulator it watches `SimulatedKeyHolder` instead. Lifecycle reaches the session as `noteBackgrounded` / `noteForegrounded`.
- **Late session grant.** In today's `UsbCardConnection.init`, a `beginSession` that times out after 10 s stays queued. A late grant then gives an exclusive session to an object nobody holds, which produces "Another app is using the key" against paw itself. `UsbCardReader` must call `endSession` in the completion handler when the grant arrives after the timeout.
- Each seam picks its simulated twin in one place, under `#if DEBUG && targetEnvironment(simulator)`: `KeyConnection.open` (card), `PlatformCopies.make` (keystore), `SimulatedClockHolder` (clock). Simulator-only Swift lives in `app/Shared/Simulator` and `app/Paw/Simulator`; real types carry no simulator members.

**Simulation controls.** Launch arguments, declared once in `LaunchArgument` (app/Shared/LaunchArguments.swift; `scripts/launch-arguments.sh` checks every one the UI tests pass is declared and read): the simulated key (`-pawSimPin`, `-pawSimTouch`, `-pawSimTouchTimeoutMs`, `-pawSimPinTries`, `-pawSimUnplugged`, `-pawSimAttachFails`, `-pawSimKdf`), Face ID (`-pawSimFaceId`), the clock (`-pawSimAwayMs`), the git host (`-pawSimRemote…`), and the harnesses (`-pawFixture`, `-pawSetup`, `-pawHostProvider`, `-pawEnclaveCheck`). The simulated key, clock and host are one per launch.
- Stand-ins for the hardware are drawn by `SimulatorStrip`, a dashed strip under a screen: the key sheet's (`simulated-touch`, `simulated-touch-timeout`, `simulated-plug-in`, `simulated-key-answers`, from `SimulatedKeyControls`), setup's, and Settings › Simulator (`sim-advance-1h`, `sim-advance-9h`, `sim-device-lock`, `sim-late-cancel-next`, …). They act on the simulated hardware; the app's models never call into the simulator, which observes them instead.

**`SimulatedKey` is mostly new work.** Today's `core/src/sim.rs` (256 lines) is short-APDU only, has no authentication key, hard-codes the auth slot's touch policy to off (`D8 = 00`), and treats an empty VERIFY as a wrong PIN that burns a try. It needs:
- an ed25519 auth slot with INTERNAL AUTHENTICATE and its own touch policy;
- correct empty-VERIFY status (`9000` when verified, `63Cx` when not, no try spent). **This must be fixed before any core code sends an empty VERIFY.**
- VERIFY P1=0xFF (de-verify);
- plug and unplug with the verified state reset;
- command chaining and extended length;
- the KDF data object;
- the variants above and fault injection mid-APDU;
- persistence of its state.

### 3.3 Key types

```rust
pub struct StoreId(String);                    // UUID; in every path, API and record
pub struct EntryPath(String);                  // "web/github.com/alice", no extension
pub struct EntryRef   { store: StoreId, path: EntryPath, name: String, folder: FolderPath, blob_oid: Oid, access: Access }
pub struct FolderNode { path: FolderPath, name: String, entry_count: u32, color: FolderColor, writable: bool }

pub enum Access {                              // per row; drives the disc and the Home sections
    FaceId { until: Timestamp },
    NeedsKey,
}
```

`Access` has two cases because v1 has two ways to open an entry. A third unlock method adds a case then; a vector of requirements now would be switched on in Swift for one real value.

**`EntryDoc`** (`model/entry.rs`):

```rust
pub struct EntryDoc { lines: Vec<Line>, trailing_newline: bool }
enum Line { Password(SecretString), Field { key, sep, value: SecretString }, Otp(SecretString), Text(SecretString) }
```

The contract is `serialize(parse(b)) == b` for every UTF-8 input, property-tested. An edit changes only the lines it touches. `login()` returns the first of `login`, `username`, `user` (case-insensitive); `url()` reads `url` or `website`. Non-UTF-8 bodies open as `OpenBody::Binary { len }`.

**`OpenEntry`** holds the long-lived plaintext in Rust:

```rust
impl OpenEntry {
    pub fn view(&self) -> EntryView;                    // keys, kinds, lengths; no secret values
    pub fn reveal(&self, field: FieldId) -> String;     // for on-screen reveal and Large Type
    pub fn copy_bytes(&self, field: FieldId) -> Vec<u8>;// for pasteboard and fill; no view-bound String
    pub fn begin_edit(&self) -> Arc<EntryDraft>;
    pub fn close(&self);                                // zeroises
}
impl EntryDraft { set_password, set_field, remove_field, rename(to); pub fn save(&self) -> Result<SaveOutcome, PawError>; }
```

`save()` serialises, encrypts to the pinned recipients (no card), writes the `.gpg` file, commits one path, and re-seals the copy if one exists.

**What "secrets stay in Rust" means.** Rust holds the only long-lived copy, in types that zeroise on drop. Swift necessarily receives transient copies that cannot be wiped: the PIN from `SecureField`, revealed text, editor field contents, `ASPasswordCredential`, and pasteboard data. The design keeps those short-lived and few:
- the PIN crosses as bytes (`submit_pin(Vec<u8>)`), and the field state is cleared on submit;
- copy and fill use `copy_bytes`, so no `String` is bound to a view;
- `reveal` is called only while the eye is on.

No claim is made that Swift-side copies are zeroised.

**FFI roots:**

```rust
impl PawCore {
    pub fn open(container: String, platform: Platform, observer: Arc<dyn CoreObserver>) -> Result<Arc<Self>, PawError>;
    pub fn stores(&self) -> Vec<StoreSummary>;       // one in v1; plural from day one
    pub fn store(&self, id: StoreId) -> Arc<Store>;
    pub fn key_monitor(&self) -> Arc<KeyMonitor>;
    pub fn copies(&self) -> Arc<CopiesHandle>;       // status(), end_all(), reload()
    pub fn setup(&self) -> Arc<SetupFlow>;
    pub fn app_event(&self, event: AppEvent);
    pub fn present(&self, error: PawError) -> ErrorPresentation;
}
impl Store {
    pub fn home(&self) -> HomeSnapshot;              // attention, face_id rows, recent_needs_key rows, folders, sync footer
    pub fn folder(&self, path: FolderPath) -> FolderSnapshot;
    pub fn search(&self, query: String) -> Vec<SearchHit>;
    pub fn open_entry(&self, path: EntryPath) -> OpenPlan;   // FaceId(Arc<CopyOpen>) | NeedsKey(Arc<KeyTask>)
    pub fn new_draft(&self, path: EntryPath) -> Result<Arc<EntryDraft>, PawError>;
    pub fn delete(&self, path: EntryPath) -> Result<(), PawError>;
    pub fn sync(&self) -> Arc<SyncHandle>;           // status(), pull_now(), push() -> Arc<KeyTask>, pending(), resolve(attention)
    pub fn import_recipient(&self, armored: String) -> Result<ImportOutcome, PawError>;
}
pub trait CoreObserver { fn changed(&self, what: Change); } // Store | Sync | Copies | Key | Task(TaskId)
```

Observation is "something changed, re-read the snapshot", not diffs. Snapshots carry timestamps, not countdowns: Swift animates the touch ring and "until 18:40" from a deadline. `AutofillCore` is a second, smaller root (3.11).

### 3.4 Card and key session

`CardActor` is one OS thread per process that owns the `CardSession` and runs short jobs serially: `ReadInfo`, `VerifyStatus`, `Verify(pin)`, `Decipher`, `Authenticate`, `Deverify`, `Close`. Nothing else touches the transport. No job waits on a human except the card's own touch wait inside `Decipher` and `Authenticate`. PIN entry happens between jobs, so the actor stays free for Lock now, the key chip, or another attempt while someone is typing.

`CardInfo` becomes:

```rust
pub struct CardInfo {
    ident: String, serial: String,
    decryption_fingerprint: Option<String>, authentication_fingerprint: Option<String>,
    pin_tries_left: u8,
    decrypt_touch: TouchPolicy, auth_touch: TouchPolicy,   // Off | On | Fixed | Cached | CachedFixed
    kdf_enabled: bool,
}
```

Today `card_info` reads only `KeyType::Decryption` and reduces it to a boolean. The auth slot has its own policy, and "one touch per push" depends on it.

**`KeySession`** lives from plug-in until removal.

1. paw holds the `TKSmartCard` session only while an operation runs or the sheet is up. Before releasing the reader it de-verifies the card (VERIFY P1=0xFF), so another app cannot ride paw's PIN.
2. Because of step 1, "PIN once per plug-in" is delivered by a zeroising in-memory copy held by `KeySession`, tagged with the card's serial. It is never written to disk, the keychain, logs or the extension.
3. On the next operation: connect; confirm the card's decryption fingerprint equals the enrolled key's; confirm the serial equals the one the held PIN belongs to (a different serial with the same fingerprint is a duplicate key: valid, but the PIN is asked); send an empty VERIFY to learn whether the card is already verified; if not, VERIFY with the held PIN.
4. The held PIN is never auto-retried. If the card rejects it, it is dropped and the user is asked. With zero tries left paw does not send VERIFY.
5. **KDF.** paw reads the KDF data object. If KDF is enabled, a VERIFY with the raw PIN would fail and spend a try, so v1 refuses before asking for a PIN (2.11). Implementing KDF is a later, contained change in `card/openpgp.rs`.
6. The PIN is dropped on: reader removal, Lock now, any wrong-PIN response, app termination, and when paw comes back after more than 15 minutes in the background.

**Why the PIN survives a device lock.** The owner's decision is "once per plug-in session". Phones auto-lock within a minute or two, so dropping at lock would mean typing a long PIN on almost every use. Unlocking the phone already needs the passcode or Face ID, and the card still demands a touch per decrypt. The cost is stated in 5.2: someone who can unlock the phone, with the key still plugged in and paw's process alive, needs no PIN.

**What cannot be promised.** A suspended process runs no code. The background limit is therefore checked when paw next runs, before any card job, and that makes the PIN unusable after the limit. The bytes may remain in the suspended process's memory until then. Unplug while suspended is detected the same way on return (reader absent, or the reader list changed). A replug that happened entirely during suspension may go unnoticed; this is harmless, because it is the same key on the same phone in the same process. **OWNER** (8.1.2): the 15-minute limit, or a stricter rule.

**UNPROVEN:** whether a YubiKey honours VERIFY P1=0xFF over CryptoTokenKit, whether `endSession()` itself resets verification, and whether an empty VERIFY reports status as the specification says. The fallback de-verify is selecting another applet. If neither works, paw holds the exclusive session while foregrounded and tells the user to unplug when done. `KeySession` is not written until the diagnostic runner (section 7, M1a) has answered these on the device.

### 3.5 State machine: `KeyTask` and `UnlockAttempt`

One key operation is two cooperating machines with one face.

- **`UnlockAttempt`** is the card half. It ends when the card has produced what was asked: a decrypted session key, or a signature.
- **`SyncOp`** is the network half of clone, fetch and push. It owns connection, progress and network cancel.
- **`KeyTask`** is the FFI object the sheet binds to. It owns one attempt and at most one op, and reduces them to one `SheetStage`. Swift never combines two states.

```rust
pub enum KeyPurpose { OpenEntry(EntryPath), Clone, Fetch, Push { count: u32 }, TestKey, Handoff(EntryPath) }

pub enum UnlockState {                       // internal
    WaitingForKey, Reading,
    NeedPin { request: PinRequest },         // min_len, max_len, tries_left, previous_was_wrong
    Verifying,
    Ready,                                   // PIN verified; card idle, waiting to be asked
    KeyBusy,                                 // an earlier command still occupies the card
    WaitingForTouch { deadline: Timestamp },
    TouchTimedOut, Removed, Done, Failed { error: PawError }, Cancelled,
}

impl KeyTask {
    pub fn snapshot(&self) -> KeySheetSnapshot;   // title, stage: SheetStage, error: Option<ErrorPresentation>
    pub fn submit_pin(&self, pin: Vec<u8>);
    pub fn confirm_host(&self);
    pub fn retry(&self);
    pub fn cancel(&self);                         // cancels both halves
    pub fn take_entry(&self) -> Option<Arc<OpenEntry>>;
}
```

`SheetStage` is the table in 2.3: `PlugIn | Reading | Pin{..} | Verifying | KeyBusy | Touch{deadline} | ConfirmHost{..} | Working{label, progress} | Done | Failed | Cancelled`.

**Open an entry:**

```
WaitingForKey ─(reader present)─► Reading ─► [wrong key / no decryption key / not OpenPGP / KDF ─► Failed]
Reading ─► NeedPin (skipped if KeySession's PIN verifies) ─submit─► Verifying
Verifying ─wrong─► NeedPin{tries_left-1, previous_was_wrong} ─tries 0─► Failed(PinBlocked)
Verifying ─ok─► Ready ─► WaitingForTouch (skipped if policy off) ─touched─► Done
WaitingForTouch ─timeout─► TouchTimedOut ─retry()─► WaitingForTouch
any card state ─reader removed─► Removed ─re-plug─► WaitingForKey
any state ─cancel()─► Cancelled
```

**Clone or push (SSH auth through the card).** No human wait happens while the server's login timer is running, except the touch.

| Step | Card (`UnlockAttempt`) | Network (`SyncOp`) | Sheet |
|---|---|---|---|
| 1 | `Reading`: card info and the auth public key, no PIN | idle | `Reading` |
| 2 | idle | Probe: connect, verify host key, send the public-key query, disconnect unless the PIN is already held | `Working` "Connecting to github.com…" (`ConfirmHost` for an unknown host) |
| 3 | `NeedPin`, `Verifying`, `Ready` (skipped when the PIN is held) | no connection open | `Pin` |
| 4 | idle | Connect (or reuse step 2's connection when step 3 was skipped), host key, auth request | `Working` |
| 5 | `WaitingForTouch` while INTERNAL AUTHENTICATE runs | awaiting the signature | `Touch` |
| 6 | `Done` | upload-pack channel; replay; receive-pack channel; retries on the same connection | `Working` "Sending…" |

- A server that refuses the key is found at step 2 and costs no PIN try and no touch.
- Offline is found at step 2.
- The attempt ends at "signature produced". Progress and cancel after that belong to `SyncOp`; `KeyTask::cancel` reaches both.
- `ssh.rs` never blocks its runtime on the card. The signer returns a oneshot receiver that the `CardActor` completes, so keepalives and disconnects are handled during a 15 s touch wait.

Rules:
- The wrong-key check precedes the PIN: the card's decryption key ID is compared with the entry's recipient key IDs. A PIN try is never spent on a key that cannot do what was asked.
- On `Done` for `OpenEntry`, the copy has already been added. For `Handoff` the copy is ordinary or single-use (2.10).
- Attempts serialise on the `CardActor`. The sheet belongs to the foreground task.
- `Touch` is reported only after the actor has sent this attempt's command. If a cancelled command is still waiting on the card, the new attempt is `KeyBusy`. Otherwise the user's touch would release the cancelled entry and be thrown away.
- With a `Cached` touch policy, one touch can cover a second command within about 15 seconds. paw cannot prevent that; S1 says so, and rule 2 of 5.6 is weakened for such keys.

**Outcome (2026-11-22, `core/src/key/`).** Built smaller than the above, tested on Linux against the simulated key only:
- `KeyPort` (held for the app's life: `plugged_in`, `unplugged`, task constructors) makes one `KeySession` per plug-in. The session reads identity and both touch policies without a PIN, holds the verified PIN in zeroising memory, asks the card with an empty VERIFY whether it is still verified and re-verifies silently if not, and drops the PIN on a refusal (one try), on removal, on `forget_pin`, and after 15 minutes in the background (injected `Clock`). If paw holds no PIN it asks for one even when the card says it is verified.
- There is no `CardActor` thread and no `UnlockAttempt`/`SyncOp` split. Each `KeyTask` runs on its own thread; a per-session gate lets one card job (SELECT, PIN check, decipher or sign) run at a time, and a task waiting for it shows `Busy` (queue, not refuse). PIN entry happens between jobs.
- Stages are `PlugIn | Pin{tries_left, forgot_after_background, last_error} | Busy | Touch | Working{progress} | Done | Failed | Cancelled`. No `Reading`, `Verifying`, `ConfirmHost` or `TouchTimedOut`: those show as `Working`, the host prompt stays in setup, and a touch timeout fails the task (`TouchTimeout`, PIN kept) so that "Try again" is a new task that opens at `Touch`.
- `Touch` is reported by the session's transport wrapper as PSO:DECIPHER or INTERNAL AUTHENTICATE is handed to the reader; for a cached or unknown policy only if the card is still waiting 400 ms later.
- A sign-in task covers the key half only: it verifies the PIN before the connection opens, then hands Rust callers an `SshSigner` that signs through the session; the caller reports progress and ends it. Sync and setup are not yet rewired to it.
- Errors are a new `KeyError`; status words are read in `key/status.rs`. A security-status refusal (`6982`, `6985`) of DECIPHER or INTERNAL AUTHENTICATE is resolved by asking the card whether the PIN is still verified and by how long the refusal took (below, "Touch timeout versus lost verification"). Any reader failure is `KeyRemoved` and ends the session.
- Not built: the enrolled-fingerprint and serial check before the PIN is sent, the KDF refusal, `ErrorPresentation`, `Handoff`.

**Outcome (branch `core-setup`).** Setup's download and sync's send and refresh now run through sign-in tasks (`*_with_key`), with the network work in the task's thread, progress as `Working`, and cancel honoured between network steps. The enrolled-key check is built (`key/enrolment.rs`): setup records the key's identity in `enrolled-key`, and a `KeyPort` given it (`load_enrolled_key`, or set by setup itself) fails every task on another card before any VERIFY. Keys match by their decryption and authentication fingerprints; the serial is recorded and shown but not compared, so a duplicate key is accepted and asked for its PIN, as above. Because `KeyError` cannot gain a variant without breaking Swift's exhaustive switches, the refusal is `NotForThisKey`, naming both keys in its text, and `KeyTask.key_mismatch` (and `KeyPort.key_mismatch`) return both identities for "This is a different key from the one this store was set up with". `replace_enrolled_key` records the plugged-in key; it does not yet check that the new key opens the store or re-pin it for saving. The KDF refusal, `ErrorPresentation` and `Handoff` are still not built.

### 3.6 State machine: sync

```rust
pub struct SyncStatus {
    pub phase: SyncPhase,
    pub pending: u32,
    pub last_checked: Option<Timestamp>,
    pub has_device_key: bool,
    pub attention: Vec<Attention>,   // HostKeyChanged | RemoteReplaced | RecipientsChanged{folder} | NeedsReencrypt{path}
                                     // | PushAuthFailed | PullAuthFailed | HandoffRequested{path} | Unpushed{count} | KeptBoth{path, case}
}
pub enum SyncPhase {
    Idle, Fetching, Integrating, Pushing { pending: u32 },
    Offline { retry_at: Timestamp }, Paused { reason: PauseReason },
}
```

```
Idle ─(foreground, timer, BGAppRefresh, pull-to-refresh, after save)─► Fetching   (device key; skipped if none)
Fetching ─network error─► Offline (backoff 30 s → 1 h, reset on foreground)
Fetching ─auth refused─► Idle + Attention::PullAuthFailed
Fetching ─remote head not a descendant of last seen─► Paused{RemoteReplaced}
Fetching ─host key mismatch─► Paused{HostKeyChanged}
Fetching ─ok, remote moved─► Integrating ─► fast-forward, or replay ─► rescan ─► Idle
user taps Send ─► KeyTask{Push} ─► Pushing ─ok─► Idle
Pushing ─rejected non-fast-forward─► fetch + replay + push again on the same connection (at most 3 loops, then Failed)
Pushing ─key removed / touch timeout─► Idle (queue intact; the sheet shows the error)
```

**Replay instead of merge.** The local branch is always `origin tip + one commit per change`, and every paw commit touches one path (two for a move). When the remote moves, the engine builds a fresh chain on the new tip by re-applying each commit's tree edit with its original message and timestamp. Blobs are already ciphertext, so no decryption and no three-way merge are involved. Replay onto a replaced history uses the same code.

**Same path changed on both sides.** The local commit is rewritten during replay according to the four cases in 2.9: write to "`<name> (iPhone copy)`", keep as an add, drop, or nothing. Each produces an `Attention::KeptBoth` row. No user decision and no parked state.

**Recipient check during replay.** For each queued path the engine compares the governing `.gpg-id` blob at the old base and at the new tip. If it differs, the queued ciphertext was encrypted for the old recipients, and pushing it would break rule 8 of 5.6. That commit leaves the chain and moves to `refs/paw/parked/<n>` with `Attention::NeedsReencrypt`. Resolving it means opening the parked blob with the key and saving it again, which produces an ordinary commit for the new recipients (after the user accepts them). This is the only use of parked refs.

**Background.** `BGAppRefreshTask` runs `pull_now()` with the device key only, under a 25 s budget. It never pushes, never prompts, never touches the card and never touches `window/`. It rewrites `index.bin` and the AutoFill identity store if the tree changed.

**Outcome (branch `sync-integrate`).** Built in `core/src/sync/` (`integrate`, `queue`, `operation`, `hosts`) and `core/src/setup/`, tested against two clones of one bare repository behind the in-process SSH server. Differences from the above:
- The local copy is named "`<name> (from this iPhone <YYYY-MM-DD>)`" (then "` 2`", "` 3`"), dated by the local commit's author time, not "(iPhone copy)".
- A push refused because the server moved is retried once on the same connection, not up to three times; a second refusal is `SyncError::RemoteKeptMoving`. One sync is one login (asserted: one signature, including the retry).
- "The last tip seen" is the ref `refs/paw/last-seen/<branch>`, written by clone, integrate and push.
- Local commits that are not one-entry paw commits stop integrate with `SyncError::NotPawHistory`; uncommitted working-copy changes with `UncommittedChanges`. Both change nothing.
- Not built yet: parking a queued change whose `.gpg-id` changed (`NeedsReencrypt`, `refs/paw/parked/`). `.gpg-id` changes are reported (`IntegrateReport.recipients_changed`), and integrate proceeds.
- `SyncStatus`/`SyncPhase`, backoff and the device key are not built; sync is a function the app calls with the card.

### 3.7 Recipients and encryption

```rust
pub struct RecipientSpec(String);            // one .gpg-id line, uninterpreted
pub enum Resolution { Ready(Vec<EncryptionKey>), Missing { specs: Vec<RecipientSpec> } }
pub struct RecipientBinding { spec: RecipientSpec, encrypt_to: EncryptionKey, from_card: bool }
```

- `.gpg-id` names the primary fingerprint, but the card holds only subkeys. Setup binds them empirically: the S4 check decrypts one entry and confirms its recipient key ID equals the card's decryption subkey. If recipients are hidden, binding waits for the first successful decrypt.
- **Several recipients.** Imported certificates (2.12) bind the lines whose fingerprint they match; a certificate's primary fingerprint, or any subkey fingerprint, may match a line. If exactly one line is left unbound and the S4 check succeeded, that line is bound to the card. A line that is a short key ID or an e-mail address is bound only after the user confirms the full fingerprint shown.
- The card's decryption public key, imported certificates and each `.gpg-id`'s SHA-256 are pinned in `keyring.json`. Add then needs no key plugged in.
- `recipients_for` is all-or-nothing. If any line lacks a binding, or the governing `.gpg-id` does not match its pin, the result is `Missing` and the folder is read-only in paw (`FolderNode.writable == false`). Reading still works.
- New entries are SEIPD v1 with AES-256. On read the core rejects any message that is not public-key encrypted with integrity protection, caps input at 1 MiB, and caps decompressed output.

Certificate import is in v1 because a second recipient (a backup key) is the usual arrangement for a hardware-only key, and without it "add and edit entries" silently does not exist for such a store. **OWNER** (8.1.3): if the owner's `.gpg-id` has one line, import can slip to after M5 without changing anything else.

### 3.8 Face ID copies

```rust
impl CopyCache {
    pub fn status(&self) -> CopiesStatus;      // Off | On { entries: Vec<(Oid, Timestamp)> }
    pub fn add(&self, entry: &OpenEntry, kind: CopyKind) -> Result<(), PawError>;   // Lasting | SingleUse; never prompts
    pub fn open(&self, blob_oid: Oid) -> Result<OpenEntry, PawError>;               // Face ID
    pub fn check(&self, blob_oid: Oid) -> CopyLiveness;   // Live { until } | Ended(reason); called before every read
    pub fn sweep(&self);                       // delete expired files; foreground only
    pub fn end_all(&self);                     // Lock now
}
```

- **Key.** The first copy creates Secure Enclave key `seal.<key-id>` with `BiometryCurrentSet`, and writes `window/key.json`. Creation is race-free without a lock: the manifest is published with a create-exclusive link. If the app and the extension both try, the loser deletes its Secure Enclave key and uses the winner's.
- **Add.** Each copy gets a random 32-byte data key and is sealed in Rust with ChaCha20-Poly1305. The associated data is `(format version, store id, blob oid, key id, kind, sealed-at wall time, sealed-at continuous time, boot id, deadline)`. Only the data key crosses to Swift, for `DeviceKeystore::wrap`. The file is `window/<blob-oid>.seal`.
- **Keyed by content, not path.** The blob id identifies the exact ciphertext. A rename keeps the copy. An edit or a pull that changes the entry orphans it.
- **Open.** `check()` first. The caller supplies the blob id of the entry's current `.gpg` file; the app takes it from the tree, and the extension hashes the one file it is about to use (3.11). Then `unwrap` raises Face ID with a fresh `LAContext` and no reuse. A `SingleUse` copy is deleted before its plaintext is returned.
- **Liveness.** A copy is ended if any of these holds:
  - the wall clock is at or past its deadline;
  - the wall clock is behind the stored high-water mark (clock set back): every copy ends;
  - `boot_id()` differs from the one sealed in: every copy ends. Comparing continuous-clock values alone misses a reboot once uptime exceeds the stored value;
  - continuous time elapsed since sealing is at or past the copy's length.
- **End all** (Lock now, turning copies off, biometry change, clock rollback, reboot): delete the Secure Enclave key first, which makes every copy undecryptable in one step, then the files, then `key.json`.
- **Sweep** deletes expired files at app launch, on every foreground, at extension launch, and from a foreground timer at the next deadline. When no copies remain the key is deleted too. Between a deadline and the next sweep the file still exists; every read path fails closed. Background tasks never run the sweep: the files are `Complete`-protected and unreadable while locked.
- **Invariant.** No copy outlives the configured length measured from the event that put its plaintext on this phone: a key touch, or the user typing a new entry.

Decisions inside this section:
- *Per-entry deadlines* won over one shared end time. A shared, fixed end makes an entry opened five minutes before the end vanish five minutes after a touch, with no warning, and a window that starts at 08:00 leaves evenings all-key. A sliding shared end lets a copy made at hour 0 live to hour 15. Per-entry deadlines avoid both and keep the invariant. The cost: expiry of a single copy is a file delete plus a read-time check, not a key delete. **OWNER** (8.1.1).
- *Sealing format.* AEAD in Rust with real associated data won over sealing the whole payload in Swift with ECIES: plaintext never crosses the FFI to be sealed, and the format is tested on Linux.
- *Key access class.* `WhenPasscodeSetThisDeviceOnly`, so removing the passcode deletes the key.

**Outcome of the core half (branch `face-id-copies`).** Built: `core/src/copies/` (`CopyCache`, the on-disk format, the `DeviceKeystore` and `Clock` seams), `SimKeystore` and `SimClock` in `sim/copies.rs`, and `PassStore::open_entry_keeping_copy`, `open_entry_with_face_id` and `face_id_status`. Where it differs from the sections above:

- **Keyed by entry path plus a content id, not by blob oid.** The file is `window/<HMAC(index key, path)>.copy`; the content id (SHA-256 of the `.gpg` bytes) is sealed inside and checked on every read, so a pull still makes a copy stale. A rename loses the copy.
- **`DeviceKeystore` is `seal`, `open`, `index_key`, `reset`**, not `create_key`/`wrap`/`unwrap`/`sign`. The data key is still the only thing sealed by the Secure Enclave. `index_key` is new: a keychain secret usable without Face ID that names the files and seals each copy's metadata, so status and the Home list are authenticated without a prompt and file names say nothing. There is no `key.json`; Swift creates keys on first use.
- **No high-water-mark files.** "Clock set back" is a copy whose sealed creation time is later than now. A copy also ends once its length has passed on the monotonic clock, so a smaller step back cannot lengthen it.
- **Every call sweeps.** Each operation reads every copy's metadata, applies the end-all rules and wipes what has ended, before any prompt. Ending all resets the keystore first.
- **One "key gone" error.** Enrolment change and passcode removal both arrive as `KeystoreError::KeyGone` and `PawError::FaceIdChanged`; telling them apart is left to Swift.
- **Boot id.** `kern.bootsessionuuid` is unreadable from the iOS 18 sandbox; the `Clock` docs describe a POSIX-semaphore restart check instead. Not yet tried on a device.
- Not built: `SingleUse` copies and hand-off, re-sealing on edit, per-entry "Always ask" (the caller passes a zero length), `CoreObserver`, Darwin notifications, store id in the metadata.

### 3.9 Search and service matching

```rust
pub fn search(&self, query: &str, limit: u32) -> Vec<SearchHit>;     // names and folders only
pub fn match_services(entries: &[EntryRef], services: &[String], pairings: &[Pairing]) -> Vec<MatchedEntry>; // AutoFill: path-derived domains + user pairings
```

Both run in memory over `NameIndex`. `index.bin` is a versioned compact dump written by the app after each scan, so the extension never walks the tree. It records the HEAD commit id it was built from. On launch and on foreground the app compares that with the repository's HEAD and rebuilds the index if they differ (a background pull killed at its budget, or a crash between checkout and index write, leaves it stale). The index is a lookup aid only: nothing security-relevant trusts its blob ids (3.11).

**Outcome of M2's core half (branch `store-model`).** Built: `store/` (tree, `.gpg-id`, safe reads, search, service matching), `recipients.rs` (who an entry is for, from its clear-text headers), `entry.rs` (`EntryDoc`), a UniFFI `PassStore` and `OpenedEntry`, and a fixture store beside the simulated key. Where it differs from the sections above:

- **`Access` means "can this key open it"** (`CanOpen`, `MightOpen` for hidden recipients, `NotForThisKey`, `Unreadable`), not `FaceId | NeedsKey` as in 3.3. The Face ID state needs its own name when M3 adds it.
- **Decrypting asks the card only for the session key addressed to it.** The library call the spike used tried every recipient packet in order: an entry for two keys cost a touch per wrong packet, and failed outright unless the card's packet came first. The key task's decrypt (`key/decrypt.rs`) refuses an entry for other keys before the PIN is sent.
- **No `NameIndex` or `index.bin` yet.** Search and matching run over the scanned entry list (5,000 entries scan and search well inside the test's bound). The index file belongs with the extension in M3b.
- **Search order** is: whole name, name prefix, word in the name, folder word, inside the name, inside a folder; then shorter path, then alphabetical. There is no recency yet (2.6), because nothing records it.
- **Service matching uses the Public Suffix List** (`psl` crate, compiled in). A path part must be a host name to match: an entry called just `github` matches no site, because anyone can register `github.example`. User-made associations (3.9) are not built.
- **`PassStore` is an immutable snapshot**; the app opens a new one after a sync or an edit. `PawCore`, `StoreId`, `EntryPath` and the observer are not built. Paths cross the boundary as strings and are checked on every read.
- **`OpenedEntry` has no `copy_bytes` or `begin_edit`.** Secrets cross as `String` on explicit calls only. `EntryDoc` can already set the password or a field byte-exactly; saving is M5.
- **`PawError` is still flat**, with `Store`, `NotText`, `NotForThisKey` and `InvalidField` added. The structured errors of 3.14 are M1b's.
- **The fixture store lives in `sim/fixture.rs`**, under whatever gate `sim` gets (5.5). Today that is none: the split into `paw-sim` is still to do.
- Not checked on read: that the message is integrity protected (3.7), and decompressed size. ASCII-armoured entries are reported as unreadable.

### 3.10 Git library

**Decision rule, settled by a Linux spike before any `sync/` code:**

1. **Hour one:** `cargo build --target aarch64-apple-ios` of russh on Linux with each crypto backend. Its backends (aws-lc, ring) contain C and assembly.
2. **If that fails,** the core cross-compile must move to the `macos-26` runner anyway. Choose `git2` (libgit2) with a custom transport over russh at once. Pure-Rust git has no remaining advantage.
3. **If it passes,** time-box two days on the `gix` path (clone, fetch, tree edit, commit, checkout, plus a hand-written fast-forward push) and compare it with `git2` + russh transport on the same exit test. Choose gix only if all three custom pieces (send-pack, a caller-supplied transport, checkout) work inside the time box. Otherwise `git2`.

Exit test for either: from Linux, clone a GitHub repo, commit, and push with the SSH signature produced by `SimulatedKey`'s auth key, with the fetch and the push on one connection; the same code builds for iOS.

What the choice actually costs, checked against the repo:
- `.github/workflows/build.yml` runs `cargo test` and `scripts/build-core.sh` on `ubuntu-latest`. That script cross-compiles `aarch64-apple-ios` and `aarch64-apple-ios-sim` there and hands the libraries to the `macos-26` jobs.
- The shipped app is already built on `macos-26` by the `device` job, unsigned, and only signed on the owner's Linux machine (`scripts/install.sh`). So libgit2, which needs the Apple SDK, moves one step (the core cross-compile) to the Mac runner. `cargo test` stays on Linux against system libgit2. The Linux dev loop for logic is unaffected.
- What gix would preserve is a locally checkable iOS build of the core and a core with no C. That is worth two days of trying, not three custom subsystems.

Constants either way:
- Signing happens on the card or the Secure Enclave, so the SSH layer needs a custom signer. libssh2 is not used.
- After replay, a push is always a fast-forward of one branch with a handful of new objects.
- There is no `GitBackend` trait. All library use stays inside `sync/` behind five functions (`clone`, `fetch`, `integrate`, `commit`, `push`), so a swap is one module.
- Add `cargo deny` to make any new crate with a C build dependency an explicit decision.

**Outcome of the spike (branch `git-spike`).** Step 2 applied: russh needs ring or aws-lc for its AEAD ciphers and has no pure-Rust backend, and both fail on Linux for want of `xcrun`. The stack is `git2` 0.21 (vendored libgit2 1.9, no TLS, no libssh2) with a smart subtransport over `russh` 0.65 (`ring` backend). What the spike showed, against an in-process SSH server in front of the real `git-upload-pack` and `git-receive-pack`:

- russh's external signer works for ed25519 with the card, and it sends the public-key query first. A key the host does not know is refused before the signer runs: no PIN try, no touch. `Connector::probe` (4.2) is therefore a by-product of `open`, not a separate round trip.
- One login per connection, and each git service is a channel on it. Clone is one signature; fetch then push on one connection is one signature. Not yet tried against GitHub itself.
- The data to sign for user `git` with an ed25519 key is 146 bytes, so it fits a short APDU. Risk 5 in 8.2 only bites for long user names; the simulated card accepts chaining and extended length regardless.
- The signer trait is synchronous (`public_key`, `sign`). The connection runs russh on its own worker thread and the caller's thread blocks in `sign`, so the oneshot in 4.2 is not needed until `CardActor` exists.
- libgit2 picks transports by URL scheme from a process-wide table. The connection is handed to it through a thread-local for the duration of one call.
- The host key algorithms offered must be narrowed to the pinned key's type, or a host with several keys may present another one and fail the pin.
- libgit2's checkout follows a folder symlink in the working copy and treats paths as patterns unless told otherwise. `commit` builds from the tree, refuses paths through links, and checks out literally. The entry reader (M2) must likewise not follow links out of the store.
- `cargo test` builds the vendored libgit2 rather than a system one, so Linux and iOS run the same C.
- Still open: ECDSA P-256 through the external signer (device key); two channels on one connection at GitHub; the macOS build of the iOS libraries, which has not been run.

**UNPROVEN** (answered by the spike): russh cross-compiling from Linux; russh's external-signer path for ed25519 (card) and ECDSA P-256 (device key); russh exposing the public-key query; two session channels on one connection against GitHub; for gix, a caller-supplied blocking transport and a practical pack writer.

### 3.11 AutoFill extension

```rust
impl AutofillCore {
    pub fn open(container: String, platform: Platform) -> Result<Arc<Self>, PawError>;
    pub fn matches(&self, service: ServiceId) -> Vec<AutofillRow>;     // from index.bin
    pub fn search(&self, query: String) -> Vec<AutofillRow>;
    pub fn plan(&self, path: EntryPath) -> AutofillPlan;
    pub fn open_from_copy(&self, path: EntryPath) -> Arc<CopyOpen>;
    pub fn open_with_key(&self, path: EntryPath) -> Arc<KeyTask>;
    pub fn key_reach(&self) -> ExtensionKeyReach;                      // Unknown | Works | Blocked
    pub fn request_handoff(&self, path: EntryPath, service: ServiceId);
    pub fn cancel_handoff(&self);
    pub fn associate(&self, path: EntryPath, service: ServiceId);
}
```

Behaviour is one function, `autofill::plan(row, reach) -> AutofillPlan`, tested on Linux:

| Entry state | Key reach | Plan |
|---|---|---|
| Covered (lasting or single-use copy) | any | Face ID, fill |
| Needs key | `Works` | Choice screen: `KeySheet` in the extension, or hand-off |
| Needs key | `Blocked` or `Unknown` | Hand-off screen |

- **Staleness.** The extension never opens git, and `index.bin` can lag the working tree. Before using a copy or sealing a new one, the extension reads the one `.gpg` file and computes its git blob id (SHA-1 over `blob <len>\0` plus the bytes). A copy is served only if a seal exists for exactly that id. An old password can therefore never be served for a changed file.
- **Reach** is probed deliberately, never during a fill. The probe runs at S6 and from Settings › AutoFill › **Check now**, with the key plugged in, by asking the user to open the provider once ("Open the key icon in any password field, then come back"). **UNPROVEN:** whether a cheaper trigger exists that runs code in the real extension process. Rules:
  - probe only when `slotNames` is non-empty;
  - record only definitive outcomes: a session granted and a SELECT answered is `Works`; a session that hangs past 3 seconds with a reader listed is `Blocked`; "no key" records nothing;
  - key the record by iOS build and by request type (password or passkey), since the one hang report was passkey-specific;
  - an in-extension key attempt that hangs at fill time downgrades the record to `Blocked` and falls through to the hand-off screen.
  
  The design does not depend on how the device spike turns out and adapts if an iOS update changes the answer. If the spike shows the extension can never reach the card, remove `com.apple.security.smartcard` from the extension (present today in `app/project.yml`) and fix reach to `Blocked`.
- **Limits.** `AutofillCore::open` reads `config.json`, `index.bin` and `window/` only. It never opens the repo as a git repository, never constructs a `SyncEngine`, never starts the SSH runtime, opens no socket, and never writes inside `repo/`. It writes only its own recents log, `handoff.json`, associations, and new sealed copies after its own key decrypt.
- **How the "no network in the extension" rule is enforced.** Not by a symbol scan. UniFFI's generated Swift references every exported function at initialisation (the per-function checksum calls), so with one library the sync code stays linked into the extension, and a scan for its symbols would fail from day one. The rule is structural: `AutofillCore` has no path to `sync` (a Rust module-dependency test on Linux), and a Linux test runs the whole `AutofillCore` surface under a seccomp filter (or equivalent hook) that fails on any socket call. A second library with its own bindings would make it four library variants with the simulator umbrella; rejected.
- **Memory.** `index.bin` is capped at 4 MB. A Linux test runs the `AutofillCore` path under a counting allocator and fails above 8 MB peak heap with a 10,000-entry index.
- **Root choice.** A separate `AutofillCore` won over a `ProcessRole` flag on one root, because a narrower API makes the limits structural.

### 3.12 Swift side

```
app/
  Paw/           PawApp, AppModel, Router, PlatformFactory
    Views/       HomeView, FolderView, EntryView, EntryEditor, SearchBar, QueueView, AttentionListView,
                 KeptBothView, SetupFlowView, BackgroundUpdatesView, SettingsView,
                 DiagnosticsView (today's SelfTestView plus the APDU script runner), SimStrip (simulator only)
  PawAutoFill/   CredentialProviderViewController, AutofillModel, AutofillViews
  Shared/        UsbCardReader (from UsbCardConnection), SecureEnclaveKeystore, Clipboard,
                 IdentityStoreWriter, KeySheet, KeySheetHost, GoldDisc, KeyDrawing, Theme
  Generated/     paw_core.swift (paw_sim.swift wrapped in #if targetEnvironment(simulator))
```

```swift
@MainActor @Observable final class AppModel: CoreObserver {
    let core: PawCore
    var home: HomeSnapshot
    var sync: SyncStatus
    var copies: CopiesStatus
    var key: KeyStatus
    var keyTask: KeyTaskModel?         // non-nil presents the key sheet
    nonisolated func changed(what: Change) { Task { @MainActor in self.refresh(what) } }
}
enum Route: Hashable { case folder(FolderPath), entry(EntryPath), edit(EntryPath), newEntry(FolderPath), queue, attention, keptBoth(EntryPath), backgroundUpdates, settings }
```

- One `AppModel` in the environment, no per-screen view models, no Combine, no persistence in Swift.
- **Presenting the key sheet over other sheets.** A single root `.sheet(item:)` cannot present while another sheet (the editor, a form sheet on iPad, the provider) is up, because UIKit refuses a second presentation from the same presenter. `KeySheetHost` is a view modifier, `.keySheetHost()`, applied at the root and at the root of every presented sheet. All instances bind to the one `keyTask`; only the topmost presents. `testSendNowFromInsideEditorSheet` covers it.
- iPhone: `NavigationStack` with bottom search. iPad: `NavigationSplitView`. `RootView` picks by size class; `FolderView` and `EntryView` are shared.
- There is no `paw://` URL scheme in v1. The extension hand-off uses a file in the shared container plus a Darwin notification.
- Both targets need App Group `group.com.marktoda.paw` and a shared keychain access group; the app also needs an app-only group for the deploy key. None exist in `app/project.yml` today (it has only the smart-card and credential-provider entitlements). They are created in M1, because M2 and M3 already write into the container. **UNPROVEN:** that the App Group container and shared keychain group work in the simulator with `CODE_SIGNING_ALLOWED=NO`, which `scripts/ci-test.sh` passes. Fallback: ad-hoc signing (`CODE_SIGN_IDENTITY=-`) with entitlements in the simulator job.
- Move to Swift 6 language mode now (`SWIFT_VERSION` is "5.0") while there is almost no Swift.
- Keep `app/Shared` free of UIKit imports where possible, for a later Mac app.

### 3.13 Threading, processes and cancellation

| Thread | Runs | Blocks on |
|---|---|---|
| `paw-card` (`CardActor`) | all APDU exchanges | `transmit`, up to the touch timeout |
| `paw-sync` | `SyncEngine`, plus a current-thread tokio runtime used only inside `ssh.rs` | network. Never the card: signatures arrive through a oneshot |
| `paw-bio` | spawned per copy open | `DeviceKeystore::unwrap` (Face ID) |
| caller | snapshot reads, search, `save()` | file I/O under 50 ms; Swift calls `save()` off the main actor |

- No exported call blocks on a human or the network. Every such wait is an object with states and a cancel. A blocking core call with a foreign prompt callback was considered and lost: it needs a Rust-to-main-thread blocking callback, cancel becomes awkward, and a rebuilt view cannot re-read where things stand.
- **Cancelling a card wait.** `cancel()` sets a flag and calls `CardConnection::abort()`; the actor maps the resulting error to `Cancelled`. **UNPROVEN:** that ending the `TKSmartCard` session during a touch wait returns promptly. Fallback: the sheet closes at once and the card's 15 s timeout runs out on the actor thread. Any task started in that time sits in `KeyBusy` and shows Touch only when its own command is sent (3.5). `UsbCardReader.transmit` keeps its 40 s timeout as the last defence.
- Every network read has a 30 s timeout.

**Two processes on one container.** The app and the extension share `window/`, recents, `handoff.json` and associations. `CoreObserver` does not cross processes.

- **No long-held file locks.** There is no `store.lock`. iOS kills a process that is suspended while holding a lock on a file in a shared container. Instead:
  - checkout writes each file by temp plus rename, and the extension reads exactly one `.gpg` file per fill, so it sees an old or a new whole file, and the blob-hash check (3.11) makes either safe;
  - the seal key is created with a create-exclusive manifest (3.8);
  - recents are one append-only log per process (`recents.app.log`, `recents.ext.log`), merged and compacted by the app;
  - the clock high-water mark is one small file per process; readers take the maximum;
  - `handoff.json` has one writer per direction: the extension creates and removes requests; the app only reads, and records completion by creating the copy.
- **Staleness.** The extension posts a Darwin notification after writing to `window/`, recents or `handoff.json`. The app re-reads `window/` on that notification and on every foreground (`CopiesHandle::reload`).
- **Tests.** Linux tests fork two processes, one with `PawCore` and one with `AutofillCore`, on one container: simultaneous first copy (exactly one seal key survives), a copy added by one and opened by the other, recents from both merged, a checkout racing a fill.

**Outcome (branch `store-lock`).** Nothing serialised operations on one store, and two refreshes at once failed on a ref's `.lock` file (seen on CI). There is now a short-held store lock, `core/src/lock.rs`; "no long-held file locks" still holds, and is what keeps it within the iOS rule above.
- **What it is.** Per store, an in-process readers-writer gate (keyed by the canonical state directory; a waiting writer goes before new readers) plus an advisory `flock` on `<state>/store.lock`, through `std::fs::File::try_lock`/`try_lock_shared` (no new crate, no C). A repository not at `<state>/repo` (tests) is its own store, locked in `.git/paw/`. Waits are bounded (15 s to change, 30 s to read), then fail with `StoreBusy` (a variant of every error type that can meet the lock, saying "the password store is busy"): nothing was changed and the store is not broken. The kernel drops a dead holder's `flock`.
- **`store.lock` is paw's only file lock, and it is held for local file work only.** Exclusive: publishing fetched refs, integrate, `accept_replaced_remote`, commit (save, delete, rename, move: the whole check-then-commit), discard/undo/discard all, recording a push, clone's init and checkout, setup's removal of `repo/`, its keyring and enrolment writes, every keyring change in `StoreWriter`, `replace_enrolled_key`, `known_hosts` changes, the GitHub link. Shared: `PassStore::open` (the listing), `read_entry`, recipients, the queue, held-back and refused-remote reads, `folder_recipients`, `recipient_scopes`. Checked by reading every holder: none holds it across a transfer, a card exchange (PIN, touch, decrypt) or a callback into Swift (progress, `SetupEvents`, keystore, vault); Face ID copies are written after a save has released it.
- **Never held across the network or a person.** A fetch (SSH or GitHub HTTPS) writes objects and its refs into a private namespace, `refs/paw/fetch/*`, with no store lock, under an in-process fetch lock (one fetch writes packs at a time; nothing is written into `FETCH_HEAD`). The store lock is then taken to move them to `refs/remotes/origin/*`, check, integrate and check out. A push sends an explicit commit (`<oid>:refs/heads/main`) with no lock, then takes it to move `origin/main` and last-seen forward; if a push that landed cannot get the lock to record that within 2 minutes it still reports success, and the next fetch heals the refs. A refused push reports its own error, never "busy". Logins (PIN, touch) happen before any lock. A send holds an in-process push lock from before it reads its tip until it has recorded the result, and marks the commit going out (`refs/paw/pushing/<branch>`), so a discard of a change that is on the wire is refused ("being sent right now") instead of appearing to work; a mark whose push is gone is removed by the next discard or fetch.
- **No deadlock.** Functions that change a store take `&Exclusive` (compile-time evidence); public entry points take the lock. A thread that already holds the lock gets a nested guard; asking to write while only reading, or to take the fetch or push lock while holding the store, is an immediate error. Order: push, fetch, store.
- **Crashes.** Taking the exclusive lock removes git `*.lock` files in `.git`, `refs/`, `logs/` (all written only under it), except `shallow.lock`; the fetch lock removes that, the private namespace's and stale pack-index locks.
- **Token refresh.** Serialised in the process, re-reading the vault after the lock; no lock between processes (one held across the refresh request could get a suspended process killed). If the app and the extension spend the same refresh token, GitHub honours one; the other, refused with `bad_refresh_token`, re-reads the vault for up to a second and uses the pair the winner kept, and signs out only if the spent token is still the kept one. A refresh after a 401 uses a token another caller already rotated instead of spending the refresh token again. Sign-out waits for a refresh in progress in its process.
- **The extension** reads entries through `PassStore` (shared lock, released before any key or Face ID work) and may save one through `StoreWriter` (exclusive for a few ms). It must never fetch, refresh the store or send: the fetch and push locks do not reach across processes. It must not hold a `StoreLock` guard itself.
- **Face ID copies** (`window/`) need no lock: every copy is written whole (unique temporary file, then rename), readers treat a missing or unreadable file as no copy, and keys come from the shared keychain (create, or read the other process's on duplicate). A reset by one process while the other writes can only lose that copy (sealed under keys that are gone, it is wiped on the next scan), never serve a wrong one; the blob-id check guards staleness. Both processes may put, open, forget and sweep.
- **Risk left:** a process suspended inside a store-lock critical section (most likely the first clone's checkout of a large store) holds the `flock` in the shared container, which iOS may punish with `0xdead10cc`. The app should run sync and save inside a background task assertion, as it already must for the network.

### 3.14 Errors

```rust
pub enum CardError {
    NoReader, Removed, Busy, Timeout, NotOpenPgp, NoDecryptionKey, NoAuthKey, AppletTerminated, KdfEnabled,
    WrongPin { tries_left: u8 }, PinBlocked, PinFormat, TouchTimeout, VerificationLost,
    Unsupported { ins: u8 }, Protocol { sw: u16, phase: Phase }, Fault { sw: u16 },
}
pub enum PawError {
    Card(CardError), WrongKey { needed: Vec<String>, have: String }, Cancelled,
    Entry(EntryError),      // NotAnEntry | NotText | UnsupportedCipher | Corrupt | TooLarge
    Store(StoreError),      // RecipientsMissing{specs} | RecipientsChanged | NoGpgId | InvalidPath | Exists | NotFound | NoMatchingRecipient
    Copy(CopyError),        // Expired | BiometryFailed | BiometryLockedOut | BiometryNotEnrolled | PasscodeRemoved | BiometryChanged | Stale
    Sync(SyncError),        // Offline | AuthRefused{kind} | HostKeyChanged{old, new} | UnknownHost | Rejected | RemoteGone | RemoteReplaced | Corrupt
    Internal { detail: String },
}
```

**Classification needs the APDU trace.** The fix is in the core's own `CardTransaction` wrapper (the `Tx` struct in `key/card.rs`): it records `ApduRecord { ins, sw, elapsed }` for every exchange, and `card/status.rs` classifies from the last records regardless of what the library returned. The same wrapper emits "touch wait started" when it sends INS `0x2A` (decipher) or `0x88` (internal authenticate), which replaces the library's touch callback and the `touch_needed` trait method.

| SW | Phase | CardError |
|---|---|---|
| `6982` | VERIFY | re-read the try counter, then `WrongPin` or `PinBlocked` |
| `63Cx` | VERIFY with data | `WrongPin{tries: x}` |
| `6983` | any | `PinBlocked` |
| `6982`, `6985` | DECIPHER / INTERNAL AUTHENTICATE | ambiguous: resolve as below |
| `6A82`, `6A86` on SELECT | connect | `NotOpenPgp` |
| `6A88` | reading key data | `NoDecryptionKey` / `NoAuthKey` |
| `6285`, `6E00` after SELECT | any | `AppletTerminated` |
| `6D00` | any | `Unsupported` |
| `6700`, `6A80` | any | `Protocol` (our bug) |
| other | any | `Fault` |

**Touch timeout versus lost verification.** Both produce `6982` on DECIPHER; `core/src/sim.rs` returns it for both too. After `6982` or `6985` on DECIPHER or INTERNAL AUTHENTICATE, in the same card job, the core sends an empty VERIFY (P2 `0x82`, no try used; `KeySession::refused`):

- `63Cx` or `6982` (not verified): the held PIN is dropped and the PIN stage comes back with `last_error: PinForgotten` ("Your key asked for its PIN again", "Enter it to go on. No try was used."). The held PIN is not re-sent: something reset the card mid-job. A second loss in the same task fails it (`Reader`, "the key lost the PIN twice"), so it never loops. A signature, which cannot ask while the server waits, fails with `PinForgotten`.
- `9000` (still verified), on a slot that can ask for a touch, after 1 s or more (`FAST_REFUSAL`): `TouchTimeout`, PIN kept.
- `9000` but quicker than 1 s: not a timeout, since a YubiKey waits about 15 s for a touch, and "touch the gold part while it blinks" would be wrong advice. It fails as `Reader` with the status word ("the key refused at once (6982)"), PIN kept; the key log has the exchange. So does a refusal on a slot that never asks for a touch.
- A card that cannot answer the empty VERIFY is judged on the time alone.

Elapsed time is measured on the real clock around the command; the simulated key's `time_out_touch` therefore answers no sooner than 1.1 s after the command, and the Linux tests that let its own timeout run use 1.1 s. **UNPROVEN** on real firmware: what a YubiKey answers after its touch timeout, and to the empty VERIFY then. The key log (Settings › Key diagnostics) records both.

**Key log** (`core/src/key/keylog.rs`). Each key session keeps a ring of its last 120 lines (about 40 commands) and each process appends the same lines to `key-log.app` or `key-log.fill` in the store's state directory (only while it exists; cut back to the last 200 lines past 40 KB; it takes the folder's protection class and goes with Remove store). A line is a card command's instruction, P1, P2, whether it carried data, status word with a short meaning, round trip and time since the session started; or a task's purpose word, whether the PIN was held when it started, both slots' touch policies, the touch watch armed, its stages and its error's name; or a session event (start, read again, end, PIN held or dropped and why). Never command or response data, the PIN or its length (nor the length byte of `61xx`/`6Cxx`, shown as such), entry or host names, or an error's text. From the PIN stage until the VERIFY that carries the PIN, lines get whole-second times, so the log does not time the typing to the millisecond; the merge keeps each file's order. The port writes to it once `set_key_log` or `load_enrolled_key` names the state directory; the latter is the call the AutoFill extension already makes, and it names the process `fill` inside an `.appex`. `key_log` merges the files by time; Settings › Key diagnostics shows it newest first with Copy log and Clear.

**Where wording lives.** The core returns `ErrorPresentation { code, params, title, body, action }` in English. The wording is then tested on Linux and shared by the app and the extension. `code` and `params` are stable, so a String Catalog can take over for localisation with the core's English as fallback. The exact strings in section 2 are the content of that table. Error variants carry fixed text or library error kinds, never entry data.

### 3.15 Data on disk

App Group container `group.com.marktoda.paw`:

```
config.json                      UntilFirstUserAuth   schema version, stores, enrolled key, remotes, copy length, opt-outs, associations, folder colours
known_hosts.json                 UntilFirstUserAuth
handoff.json                     Complete             one pending HandoffRequest
extension_reach.json             UntilFirstUserAuth   ExtensionKeyReach per iOS build and request type
stores/<store-id>/
  repo/                          UntilFirstUserAuth   working tree + .git (ciphertext only; queue = commits ahead of origin; parked refs)
  index.bin                      UntilFirstUserAuth   names only; HEAD commit id
  keyring.json                   UntilFirstUserAuth   public keys, bindings, .gpg-id pins
  recents.app.log, recents.ext.log   Complete         paths + timestamps; compacted to 50
window/
  key.json                       Complete             key id, keychain tag
  hwm.app, hwm.ext               Complete             wall-clock high-water marks
  <blob-oid>.seal                Complete             AEAD copy + wrapped data key (deadline, boot id, kind in the AAD)
```

- `UntilFirstUserAuth` is required because background pull writes the repo and index while the phone is locked. `Complete` files are touched only in the foreground and by the extension, never by background tasks. Orphan deletion and the sweep are foreground-only for the same reason.
- Everything is excluded from backup. All JSON and bin files are written with temp file plus rename.
- Copies are one set per device, not per store. Each copy's associated data carries its store id.
- Keychain:
  - `seal.<key-id>`: Secure Enclave P-256, shared group, `WhenPasscodeSetThisDeviceOnly` with `.privateKeyUsage` and `.biometryCurrentSet`.
  - `deploy.<store-id>`: Secure Enclave P-256, app-only group, `AfterFirstUnlockThisDeviceOnly` with `.privateKeyUsage` only.
  - Nothing else. No PIN and no plaintext.

**The queue is the commits ahead of origin.** There is no second database to drift. Every paw commit is a single-path change, and `PendingChange` is derived by reading those commits.

### 3.16 Testing

| Layer | Where | What |
|---|---|---|
| `model` | Linux, unit + proptest | `serialize(parse(b)) == b`; edits change only their line; path validation; generator output class |
| `store` | Linux | `.gpg-id` nearest-ancestor table; multi-recipient binding; certificate import; `match_services` table including pairings; search ranking; index round-trip and stale-HEAD rebuild; pin mismatch gives read-only |
| `crypto` | Linux | Real GnuPG fixtures (SEIPD v1, OCB, compressed, multi-recipient, hidden recipient) decrypt with `SimulatedKey`; entries written by paw decrypt with `gpg` and `pass show`; literal-only and signed-only messages are rejected; size caps |
| `card` | Linux | Trace classification per phase; empty-VERIFY disambiguation; scripted-APDU replay of real YubiKey traces from the diagnostic runner; removal mid-APDU; `abort()` unblocks; KDF refusal spends no try |
| `unlock` | Linux | Exact state sequences: happy path, wrong PIN ×3, last try, touch timeout then retry, unplug and cancel at each state, second open skips PIN, wrong key spends no try, `KeyBusy` after a cancelled touch, PIN lifetime events |
| `window` | Linux | Per-copy deadlines; key deleted before files; clock rollback; `boot_id` change; reboot with larger uptime; stale blob; single-use opens once; a crash between key delete and file delete leaves nothing openable |
| `sync` | Linux | Temp bare repos: fast-forward, replay, the four keep-both cases, re-encrypt parking on a `.gpg-id` change, replaced history with replay, non-fast-forward retry on one connection, backoff. In-process russh server: key query, card-signed and device-key-signed auth accepted, host key change refused, no human wait on an open connection except the touch |
| two processes | Linux | Forked `PawCore` + `AutofillCore` on one container (3.13) |
| invariants | Linux | **Canary test:** run "open, edit, save, copy, sync" with a unique plaintext marker, then scan every file for it. Allocator cap and no-socket test for `AutofillCore`. `is_denied(fingerprint)` for the simulated key |
| app | CI, XCUITest | One test per flow per scenario (section 2) |
| extension UI | CI | Provider view hosted in the app with `-pawHostProvider` |
| device | manual checklist | Diagnostics screen in app and extension: read key, round trip, the APDU script runner with status-word and timing export, cancel and unplug during touch, PIN persistence |

The real Secure Enclave path and the real extension process cannot be tested in the simulator. Keep each to about a hundred lines of Swift with a written checklist.

---

## 4. Unlock and git auth as pluggable providers

### 4.1 Unlock

**v1 has one unlock provider, the OpenPGP card, and no `Identity` trait.** A trait shaped as "ciphertext in, plaintext out, prompting through a callback" was considered and dropped for three reasons:
- it puts a second owner of the PIN and touch sequence beside `UnlockAttempt`;
- its blocking call would sit on the `CardActor` thread while a human types, which blocks Lock now and the key chip;
- its only product implementation would be the card, and the next likely one (NFC) does not fit the shape: PIN first, then tap, re-verify every time.

`UnlockAttempt` drives `CardSession` directly through short actor jobs. The trait is extracted when a second product provider exists, from two real cases.

What is built now so that extraction is cheap:

| Seam | What it is |
|---|---|
| Provider-neutral sheet vocabulary | `SheetStage` says "plug in, secret, presence, working", not "smart card". `PinRequest` carries lengths and tries |
| One resolver | `Store::open_entry` decides: live copy first, otherwise a `KeyTask`. A second provider adds a branch here and a case to `Access` |
| Enrolled key as a record | `config.json` holds a list `keys: [{ id: "card:<fingerprint>", kind, serials, fingerprints }]` with one element |
| Format isolation | `pgp` is imported only in `crypto/openpgp.rs`. Recipient key IDs and `EncryptionKey` are opaque outside `crypto/`. `.gpg` and `.gpg-id` names are confined to `store/` |
| Applet-agnostic transport | `CardConnection` exchanges raw APDUs and knows nothing of OpenPGP |
| Copies are not a provider | The copy cache holds plaintext copies, not a key, and sits in front of whatever unlocks |

A software decryption key exists nowhere, including tests: multi-recipient fixtures are encrypted to the simulated card plus throwaway public keys whose private halves are discarded.

| Later provider | New work | Unchanged |
|---|---|---|
| YubiKey over NFC | Swift NFC reader implementing `CardReader`; a stage order of PIN, then "Hold your key to the top of your iPhone"; every decrypt re-verifies from the held PIN because the field drops between taps. Not possible in the extension. **UNPROVEN:** that NFC presence satisfies the touch policy | `CardSession`, sheet, errors, copies |
| Software key | An rPGP secret key wrapped by `DeviceKeystore`; import or generate screen. Requires rewriting rule 1 of 5.6 as an explicit opt-in; this is when the provider trait is extracted | Lists, entry screen, queue |
| Secure Enclave key as recipient | A P-256 ECDH OpenPGP subkey whose private operation is a foreign call. **UNPROVEN:** that rPGP accepts an external P-256 ECDH provider. Adding the recipient means re-encrypting, one touch per entry | Same |
| age / passage | `crypto/age.rs`, a store-format switch in `store/`, age recipients as opaque handles. age on a YubiKey uses the PIV applet, which the transport already allows | All screens |
| Several keys or people | Already partly in v1: imported certificates. Later: certificates read from the repo, per-folder recipient editing | Sync model |

### 4.2 Git auth: `SshSigner`, `Remote`, `Connector`

Two signers ship in v1, so this abstraction is required, not speculative.

```rust
pub trait SshSigner: Send + Sync {                    // internal to sync/
    fn id(&self) -> AuthId;
    fn public_key(&self) -> SshPublicKey;             // algorithm, blob, authorized_keys line
    fn needs_user(&self) -> bool;                     // true for the card
    fn sign(&self, data: Vec<u8>) -> oneshot::Receiver<Result<Vec<u8>, PawError>>;   // never blocks the caller
}
pub struct Remote { url: String, fetch_auth: AuthId, push_auth: AuthId, host_key: HostKeyPin }
pub trait Connector: Send + Sync {                    // the network seam
    fn probe(&self, remote: &Remote, key: &SshPublicKey) -> Result<KeyAccepted, SyncError>;   // host key + key query, no signature
    fn open(&self, remote: &Remote, signer: &dyn SshSigner) -> Result<Box<dyn GitConnection>, SyncError>;
}
pub trait GitConnection { fn channel(&mut self, service: GitService) -> Result<Box<dyn GitChannel>, SyncError>; }
```

- `CardAuthKey`: `ssh-ed25519` via INTERNAL AUTHENTICATE as a `CardActor` job, completed through the oneshot. Its `UnlockAttempt` must already be `Ready` (PIN verified) before `open` is called, so the only human wait on the open connection is the touch. The touch requirement comes from `auth_touch`. **UNPROVEN:** the SSH signature payload exceeds a short APDU; which of command chaining or extended length the YubiKey accepts through `TKSmartCard.transmit` is unverified.
- `DeviceSshKey`: `ecdsa-sha2-nistp256` (the Secure Enclave only does P-256), via `DeviceKeystore::sign` on a blocking helper thread, no prompt.

One `GitConnection` serves several channels, which is what makes fetch, replay and push one touch.

The signer is called only for the SSH user-auth request of a connection paw opened itself, after the host key has been verified. No generic "sign this" call crosses the FFI, and there is no agent forwarding.

Fetch and push auth are separate fields from day one because they are cheap now and hard to retrofit. "Device key for push" later is a settings change. An HTTPS token later is a second `Connector` and a credential type beside `SshSigner`; there is no one-variant credential enum until then.

### 4.3 How each consumer uses the contracts

| Consumer | Unlock | Git auth | Prompts |
|---|---|---|---|
| Open entry (app) | Copy, else `KeyTask{OpenEntry}` | none | Face ID or `KeySheet` |
| Setup key check (S2) | card read only | `Connector::probe` with the card's public key | none |
| Setup clone | none | `CardAuthKey` through `KeyTask{Clone}` | `KeySheet` |
| Setup check (S4) | `KeyTask{TestKey}`; pins recipients on success | none | `KeySheet` at Touch |
| Save, delete, rename | none (pinned public keys) | none | none |
| Push | none | `probe`, then `Remote.push_auth` = `CardAuthKey`; fetch and push on one connection | `KeySheet` |
| Silent pull, background refresh | none | `Remote.fetch_auth` = `DeviceSshKey` | never prompts; errors become `Attention` |
| Pull without a device key | none | `CardAuthKey` through `KeyTask{Fetch}` | `KeySheet` "Update your store", from the footer row only |
| Re-encrypt held-back changes | `StoreSync.reencrypt_with_key` (`KeyPurpose::Reencrypt`), one touch per waiting blob | none | `KeySheet` |
| Hand-off (app side) | `KeyTask{Handoff}`; lasting or single-use copy | none | `KeySheet` |
| AutoFill extension | Copy; else `KeyTask{OpenEntry}` if reach is `Works`; else hand-off | none, ever | Face ID, `KeySheet`, or hand-off |
| Gold disc, Home sections | `EntryRef.access`, from the copy cache; no I/O beyond the index and `window/` listing | none | none |

---

## 5. Security model

### 5.1 Goal

The decryption key exists only on the YubiKey. Reading an entry takes the key, its PIN and one touch, and that touch releases exactly one entry. The single deliberate exception is the Face ID copy. Everything else on the phone (clone, name index, recents, device SSH key) is worth no more than a read-only copy of the git repo. paw makes one kind of network connection: SSH to the configured remote.

### 5.2 Guarantees and non-guarantees

| Adversary | paw gives | paw does not give |
|---|---|---|
| Unlocked phone, no key, not the owner's face | No entry plaintext. Enrolling a new face destroys the seal key. Cannot push. Local changes are named in the queue before the owner's key sends them | Names, folders, recents and the covered list are visible unless "Require Face ID to open paw" is on. New entries, deletes and renames can be queued. Pasteboard within its expiry. An entry left open less than 2 minutes ago is still open |
| Unlocked phone with the YubiKey | No persisted PIN. If the PIN is not held: three guesses, then the card blocks | If paw's process still holds the PIN (same plug-in, under 15 minutes in the background), each touch opens one entry with no PIN. A device lock does not clear the PIN. Phone + key + PIN is full compromise by design |
| Locked phone | Copies and their key are unreadable while locked. No notification carries an entry name | Clone, index and use of the device SSH key are available after first unlock. A held PIN or a recently open entry may remain in a suspended process's memory until paw next runs |
| Malicious sandboxed app | Cannot read the container, keychain group or memory. Third-party keyboards are refused. paw de-verifies the card before releasing the reader | Pasteboard on paste. Another smart-card app can block the PIN or imitate the sheet. A jailbroken device is out of scope |
| Someone with the repo | Contents are confidential to the recipients | Every name, folder, file size, edit time, commit message and the full history. No forward secrecy, no post-quantum protection |
| Someone who can write to the repo | Recipient pinning, including for queued changes; history-rewrite detection; hostile-input handling (no hooks, filters, submodules or symlinks; path checks; size caps); `url` cross-check before filling | Ciphertext swap between names is caught only for entries with a `url` field. Deletion, rollback to a descendant-consistent state and withheld updates are undetectable |
| Network attacker | SSH with pinned host keys; no credential offered to an unverified server; no other traffic | Sees that the device talks to the git host and when. Can block sync |
| Shoulder surfer, recording | Masked by default; re-mask; blank app-switcher cover; hidden during capture | Names on screen. Deliberate screenshots cannot be reliably blocked |
| Hostile USB reader | The PIN is sent only to a card presenting the enrolled fingerprint | That value is public and spoofable; the PIN goes in clear to whatever answers. The PIN alone opens nothing |
| Compromised build or dependency | Nothing intrinsic; mitigations are procedural (5.7) | A malicious build can exfiltrate every entry the user opens. The private key stays safe |
| Coercion | Lock now, with no confirmation and no undo; holding the side buttons disables biometrics | No guarantee |

A phone thief can queue deletes, renames and new entries, because those need no key. The thief cannot push, every local change is named in the queue before the owner sends it, and Add refuses to overwrite an existing name. Editing an existing entry needs its plaintext.

After losing a phone, the user should remove its deploy key from the git host and rotate the entries that had live copies that day. There is no remote wipe beyond Find My.

### 5.3 Mechanisms not already covered

**Face ID copies.** As in 3.8. On a device build there is no software fallback: if there is no passcode, no enrolled biometrics, or key creation fails, copies are unavailable.

**Recipient pinning.** A `.gpg-id` added, changed or removed by a pull is never used for encryption until the user accepts it in the foreground. Until then paw refuses to write in that subtree. Refusing won over "keep encrypting to the old pin" because it is simpler and cannot silently diverge from other clients. A queued change made before such a pull is parked, not pushed (3.6). An imported certificate is used only for `.gpg-id` lines whose fingerprint it matches.

**SSH host keys.**
- Bundled: every published host key (all algorithms) for github.com, gitlab.com, codeberg.org, bitbucket.org and git.sr.ht, with no prompt.
- Other hosts: trust on first use, stored per `host:port`.
- A mismatch is a hard failure with no bypass inside the connect flow. The error shows old and new fingerprints and names the exit.
- The exit is Settings › Store › "Forget server key", for bundled hosts as well. Hosts do rotate keys (GitHub replaced its RSA host key in 2023), and "wait for an app update" is not acceptable. After forgetting, the next foreground connection shows `ConfirmHost`.
- Background pull never prompts.
- Algorithms: ed25519, ecdsa-sha2-nistp256, rsa-sha2-256/512; no SHA-1 `ssh-rsa`; no compression.

**Device key.** One per device per store, in the app-only keychain group, so the extension cannot use it. Setup probes `git-receive-pack` with it and expects refusal. **UNPROVEN** outside GitHub.

**Clipboard.** Every write uses `.localOnly` and an `.expirationDate`. No "copy whole entry", no copy on open. The extension never touches the pasteboard. There is no switch to share the clipboard with other devices.

**Screen privacy.** As in 2.7. The secure-text-field screenshot trick is best effort only and never relied on. Edit fields disable autocorrection and prediction.

**Card use.** Every decipher is bound to one named entry shown before the card starts waiting. The core API takes one entry path; no call decrypts a list.

**Logging.** No log line, error string or crash annotation contains plaintext, the PIN, APDU bytes, session keys, ciphertext or pasteboard content. Names, the repo URL and usernames are `os_log` `.private` only. Secret-bearing Rust types have no `Display`, a redacting `Debug`, and zeroise on drop. The diagnostic runner's export contains instruction bytes, status words and timings only, never command or response data. APDU data tracing sits behind a cargo feature the device build script refuses to enable. No third-party SDKs.

Known limit: the HMAC state that makes a one-time code (`hmac` 0.12, which has no zeroize support) is not zeroised on drop; the secret and the code are.

**System stores.** No CoreSpotlight, `NSUserActivity`, Siri suggestions or widgets with names in v1. `ASCredentialIdentityStore` holds path-derived data and user associations. It is cleared on store removal and when "Require Face ID to open paw" is turned on.

**Outcome (branch `core-hardening`, PIN KDF).** The session path built its own VERIFY with the PIN as typed, so a card with the PIN KDF on counted a correct PIN as wrong. The session now reads the KDF data object (`F9`) when the key is plugged in (`key/kdf.rs`) and sends the iterated and salted S2K of the PIN (SHA-256 or SHA-512) that the card expects, as the openpgp-card library does on the legacy path; the simulated key has a KDF mode (`SimulatedKey.set_pin_kdf`), checked against the library's own derivation. A KDF paw does not know fails the task with `KeyError::Reader` before the PIN stage, with no VERIFY sent. Rule 5's "never to a card with KDF enabled" therefore now reads: never the raw PIN to such a card.

**Outcome (branch `core-hardening`, setup binding).** Setup's binding no longer assumes the top-level `.gpg-id` is the card's: it binds the card only to the single unsatisfied line of the `.gpg-id` that governs the entry the test decrypt opened, and only if that entry's recipient packets name the card's key (`Keyring::bind_card_for_opened_entry`). With a root `.gpg-id` naming a team key and `personal/.gpg-id` naming the owner, the owner's line is bound and the team line stays missing, which `Setup.write_readiness` reports. The exported `StoreWriter.bind_card_after_test_decrypt` keeps its top-level behaviour for now; setup does not use it.

**Outcome (branch `core-hardening`, queued ciphertext and recipients).** Rule 8's "covers ciphertext already queued" is now enforced at send: after integrating, `StoreSync.send` and `send_with_key` check every waiting change that writes an entry, and if any is not encrypted for exactly the keys its entry's governing `.gpg-id` resolves to now (as accepted in the keyring), nothing is pushed and the sync ends `Waiting`. Acceptance alone does not release it: the old ciphertext would still be pushed into history. `StoreSync.held_back()` lists each such change with who it is encrypted for, the current `.gpg-id` lines and whether they are accepted; the person accepts, saves the entry again (re-encrypting it needs the key), and discards the old waiting change. Re-encrypting automatically is left for later. The Rust-only `operation::send` keeps the old behaviour for tests; the app's paths use `send_checked`.

**Outcome (branch `held-back`, re-encrypting held-back changes).** Held-back changes are re-encrypted by hand, never automatically. Once a folder's new list is accepted, "Re-encrypt 3 changes" sits in the sync sheet's bottom actions (`sync-reencrypt`; also on the held-back screen and "Not sent yet"); the held-back row's "Review" opens the held-back screen, which links to who the new list names. It runs `StoreSync.reencrypt_with_key`, a key task (`KeyPurpose::Reencrypt`) on the key sheet: the PIN once, then one touch per change, the sheet titled "Re-encrypt 2 of 3". The core (`queue::reencrypt_each`) opens each held-back change, encrypts it for its folder's list as it resolves now, and rewrites the queue for it at once (same changes, order, messages and times; no lock held while the key works), so a cancel or a key error part-way keeps those done and a second run starts with the next. Two changes with the same waiting ciphertext (a rename) cost one touch. A change it cannot do stays held back, with its reason on the held-back screen: the new list is not accepted, paw lacks a key on it, the plugged-in key can't open it, or it doesn't open. Nothing is sent; afterwards they are ordinary waiting changes and Send applies. `reencrypt_check` says how many are ready without the key. The UI tests cannot reach this state (the simulated host's `gpgid` change keeps the same key); it is covered in `core/tests/held_back.rs` and `PawTests/HeldBackModelTests`.

**Outcome (branch `core-hardening`, download cap).** The "size caps" of 5.2 now include the transfer: each git service on a connection may send at most `DOWNLOAD_LIMIT` (256 MiB), counted as bytes arrive, before libgit2 writes the pack. Past it the clone or fetch stops with `PawError::Git` ("the git host sent more than 256 MiB ..."), which sync reports as `SyncError::Failed`; nothing is integrated. `Connection::set_download_limit` lowers it (tests use 1 MiB).

**Outcome (branch `core-hardening`, one decipher per entry).** "Card use" above is now enforced against crafted entries: both decrypt paths ask the card to unwrap at most one recipient packet per entry (`MAX_DECIPHERS`), the first that names the card's key, or the first hidden one if none does. An entry with only hidden packets for several recipients opens only if this key's comes first.

**Outcome (branch `core-hardening`, copies after an end-all).** A copy made right after a reboot or a clock set back was written under index keys from before the keystore reset that the same call caused, and was lost at once although `put` returned a deadline. `CopyCache::put` now derives the keys again after its scan.

**Outcome (branch `core-hardening`, hostile trees).** A pull could write outside the store: a link `café -> ..` followed by `café/recipients` in NFD made checkout, on a normalisation-insensitive file system, write over `<state>/recipients`. Fixed in `core/src/sync/guard.rs`, in layers. (a) Every tree is checked before it is checked out or committed: the remote's tip at clone, integrate and `accept_replaced_remote`, every commit replay rebuilds, and every tree `move_branch` checks out (so discard too). Refused: links, submodules and any mode but folder and regular file; names that are not UTF-8, hold a slash, or fail `path.rs` (`..`, `.git` in any case, with HFS+-ignorable code points, as `GIT~1`, with trailing dots or spaces, or with a `:stream`); names some file system reads as `.gpg-id` but spelled otherwise; a `.gpg-id` folder; any two paths (file or folder, at any depth) equal once HFS+-ignorable code points are dropped and NFD is applied, under any of full Unicode case folding (`unicase`, CaseFolding.txt C+F), lower-casing or upper-casing, since APFS's exact tables cannot be checked from Linux; folders 32 deep; paths over 1024 bytes; and the whole tree past 100,000 files and folders or 16 MiB of paths, where the walk stops, so a few tree objects naming one subtree many times cannot make it unbounded. Executables are allowed, because `pass` keeps executable extensions in `.extensions/`, and are checked out as 0644 with `core.filemode=false`. Not covered: trailing dots or spaces and 8.3 short names other than `.git`'s, which only Windows folds, and Turkish-specific casing (`İ` and `i` are distinct). A refusal leaves branch, working copy and queue untouched, and is `SyncError::Failed` (or `SetupError::Failed`) whose message starts "the remote contains something paw will not check out" and names the paths; `StoreSync.refused_remote()` and `Setup.refused_remote()` list them. (b) Every repository paw opens or creates gets `core.symlinks=false`, `core.protectHFS=true`, `core.protectNTFS=true`, `core.autocrlf=false` and an `info/attributes` of `* -text -crlf -ident -filter`, which outranks any `.gitattributes`. Without it, a remote `.gitattributes` of `*.gpg text eol=crlf` did rewrite ciphertext on checkout (now tested). (c) The layout is unchanged (`<state>/repo`): the app takes the path from the core, and (a) and (b) are each tested on their own, (a) on a real casefold tmpfs in a user namespace.

**Outcome (branch `github-signin`, GitHub sign-in).** Built in `core/src/github/` (Rust only, tested against an in-process simulated GitHub, `core/src/sim/github.rs`; nothing has run against the real GitHub). paw's GitHub App (`paw-password-store`, client id `Iv23liyBePbnvQPCcGRz`, no client secret anywhere) is signed in to with the device flow; `GitHubAccount.start_sign_in` / `DeviceSignIn.poll` / `cancel` honour the interval and `slow_down` (+5 s). The access and refresh tokens never reach Swift: they are one item in a new foreign trait `TokenVault` (keychain generic password, `AfterFirstUnlockThisDeviceOnly`, not synchronised, no access control, app-only group), replaced whole on every refresh so rotation is one write; a refresh GitHub answers `bad_refresh_token` forgets the sign-in ("sign in again") and touches nothing else. `GitHubAccount.repositories` lists `/user/installations` and each installation's repositories, paged, with at most 20 `contents/.gpg-id` checks (no extra permission), or `NotInstalled` with `https://github.com/apps/paw-password-store/installations/new`; `select_repository` returns the SSH address `Setup.set_address` already takes, and the key still does the first clone over SSH. `GitHubAccount.refresh_store` fetches over HTTPS (a libgit2 smart-HTTP subtransport over `ureq` + rustls/ring + bundled Mozilla roots; `git-receive-pack` refused; token only as basic auth `x-access-token` to `https://github.com` exactly, checked before each request; redirects to another origin not followed; the same 256 MiB cap) and then runs the same integrate as the SSH refresh. It refuses to run unless the store's remote is the chosen repository. `GitHubAccount.background_refresh_stoppable(seconds, stop)` is the entry point for `BGAppRefreshTask`: it never pushes or prompts, integrates only what needs no decision, and parks a refused tree, a `.gpg-id` change (not integrated at all in the background), a replaced history and held-back changes as `decisions`; it stops between steps at its deadline and is safe to kill. Departures: 5.1's "one kind of network connection" now has a second, HTTPS to github.com and api.github.com, only once the person has signed in; 4.2's "second `Connector`" is a separate subtransport, not a `Connector`; sign-out can revoke both tokens through the unauthenticated `POST /credentials/revoke` (opt-in, GitHub emails the owner) and points to `https://github.com/settings/apps/authorizations`, because removing the app's authorisation needs a client secret. Not verified: anything against the real GitHub (device flow, rotation, `/user/installations` with a user token, smart HTTP with `x-access-token` and a user token), and the keychain vault in Swift.

### 5.4 How copies weaken "hardware key required"

For a covered entry, until its deadline:
1. The sufficient factor becomes "this phone plus the enrolled face". Its security rests on Apple's Secure Enclave and Face ID, not the YubiKey.
2. Plaintext-equivalent data is at rest on the phone.
3. Expiry is software: an expired copy's file is deleted the next time paw code runs in the foreground, and refused by a check before that. The seal key itself is destroyed only when all copies end.
4. Biometrics can be compelled in ways a PIN cannot.
5. The extension process can open covered entries.
6. There is no remote revocation.

Two additions beyond lasting copies:
- A new entry is copied without any key touch, for the configured length from the save.
- A hand-off makes a 2-minute single-use copy even when the user turned copies off or opted the entry out. It exists only because the user asked for that entry in Safari and then touched the key for it.

Copies do not weaken entries that have none, the private key, the PIN, or git write access. The device passcode alone never opens a copy. Never describe copies as "locked by your key" or "hardware protected".

### 5.5 Keeping the simulated key out of device builds

The simulated key's private scalar is a constant in a public repo. A device build that could enrol it would encrypt entries to the world. Five layers, all required:

1. **Separate crate.** `sim.rs` moves to `paw-sim`. `paw-core` has no path to it.
2. **Separate slices.** The device slice of the xcframework is built from `paw-core` alone; the simulator slice from the umbrella crate. `paw-sim` carries a `compile_error!` for device targets.
3. **Separate bindings.** `paw_sim.swift` and every Swift stand-in are wrapped in `#if targetEnvironment(simulator)`.
4. **CI proof.** After the device build, a script fails the job if `nm` or `strings` finds `SimulatedKey`, `paw_sim` or the simulated private scalar in the device library or app binary. The same script asserts that the denylist (layer 5) is present in the device library.
5. **Runtime denylist on devices.** `paw-core` hardcodes the simulated public key and fingerprint. The refusal to enrol or encrypt to them is compiled in under `cfg(all(target_os = "ios", not(target_abi = "sim")))`. It cannot be unconditional: every Linux test, scenario and XCUITest enrols and encrypts to the simulated key through `paw-core`, and a runtime off-switch would be the hole this layer exists to close. The pure predicate `is_denied(fingerprint)` is unit-tested on Linux.

`SimKeystore`, the software stand-in for the Secure Enclave, is the most dangerous stand-in and sits under the same gates. The App Store "Try with demo data" path conflicts with this and needs a separate design.

**Outcome (branch `core-setup`).** Smaller than the above. The simulated hardware (`SimulatedKey`, the fixture, `SimKeystore`, `SimClock`, and now `SimRemote`) stays in `paw-core`, in `src/sim`, behind a cargo feature `sim` that is on by default, so tests, the iOS simulator and Debug device builds keep it. `scripts/build-core.sh --release-device` also builds the device library without it, with Swift bindings generated from that library (UniFFI's Swift checks every function's checksum when it loads, so the default bindings cannot be linked against a library that lacks the simulation symbols), and `scripts/check-no-sim.sh` fails if a library has simulation symbols or the fixture's passwords. Not built: a Release configuration in the Xcode project that uses those outputs, `#if DEBUG` around the Swift that names Sim types, the check on the signed app binary, and the runtime denylist (layer 5).

**Outcome (branch `release-build`).** The device build is Release, and its core has no `sim`. `scripts/build-core.sh` always builds the device library without the feature and the simulator library with it, and generates both sets of bindings: `Generated/paw_core_device.swift` inside `#if !targetEnvironment(simulator)` and `Generated/paw_core.swift` inside `#if targetEnvironment(simulator)`, matching the xcframework's slices (each with its own header), so a device build sees only the device bindings whatever its configuration. Swift that names a Sim type, and every harness, is under `#if DEBUG && targetEnvironment(simulator)`, in `app/Paw/Simulator` and `app/Shared/Simulator`; the launch arguments (`LaunchArgument`, one declared list), the gallery and the enclave check are under `#if DEBUG`. The PIN-taking exports (`decrypt`, `card_checks`, `card_touch_timeout_check`, `read_card`, `encrypt_to_card`, `PassStore.open_entry` and `open_entry_keeping_copy`, `Setup.download` and `test_decrypt`, `StoreSync.send` and `refresh`) and the spike's test screen are gone, and so is the Rust-only per-call path they used: the Rust tests drive the key tasks the app runs. The touch timeout check is a key task (`KeyPort.check_touch_timeout`) in Settings › Key diagnostics. `scripts/check-no-sim.sh` scans the device library, and with `--app` the Release app and extension binaries for the debug harnesses as well (every declared launch argument and every type in the simulator and debug folders); `--expect` runs it on the simulator library and the Debug simulator app, where every pattern must match. Settings › About shows the commit and configuration. Still not built: the runtime denylist (layer 5).

`SimRemote` is an SSH server in the process on a loopback port, serving a fresh bare repository of the fixture store with git's upload-pack and receive-pack written on libgit2 (protocol v0, no `multi_ack`, no side-band, fast-forward pushes only). It starts no `git` program, so it can run inside the app in the iOS simulator; that has been reasoned, not tried. It can refuse the key, present another host key and drop the connection halfway through a pack.

### 5.6 The never-break rules

1. **The private key and anything that substitutes for it stay on the card.** No exported key, no cached session keys, no software decryption key in any build. The only plaintext-equivalent at rest is a sealed copy.
2. **One human gesture releases one named entry.** No batch, background, speculative or list decrypt exists. (A key whose touch policy is "cached" or "off" weakens this at the card; paw says so at setup.)
3. **Plaintext reaches disk only as a sealed copy** under a Secure Enclave key with `biometryCurrentSet` and `WhenPasscodeSetThisDeviceOnly`. No passcode fallback. Edits are encrypted before they are written. The canary test enforces this.
4. **Check the deadline before every copy read and fail closed.** Face ID use never extends a copy. A clock that went backwards or a reboot ends all copies. Ending all deletes the key first.
5. **The PIN is never persisted, logged or shared with the extension, and never retried automatically.** Send it only to a card presenting the enrolled fingerprint, and never to a card with KDF enabled. De-verify the card before releasing the reader. No PIN try is spent unless the card and the server can do the thing requested.
6. **Nothing learned from decrypted content is written anywhere except a sealed copy.** Index, recents, identity store, associations, hand-off file, logs and git metadata hold path-derived names and user-approved domains only. One exception, off until the owner turns on "Suggest one-time codes" (2.10): a list of entry paths that have a one-time code, and a one-time-code identity for each of their sites. It says which entries have a second factor, never the factor; turning it off deletes both.
7. **Every pasteboard write is local-only with an expiry.** The extension never uses the pasteboard.
8. **Encrypt only to recipients pinned on this device, and never to fewer than `.gpg-id` names. This covers ciphertext already queued:** a change encrypted before its `.gpg-id` changed is not pushed. A remote `.gpg-id` change is untrusted until confirmed. Never encrypt to the simulated key on a device.
9. **Never send a credential to an unverified SSH host, and never bypass a mismatch inside the connect flow.** Card-authenticated operations are foreground and user-initiated. Background work uses only the read-only device key and only fetches. The extension opens no sockets. paw contacts no host other than the git remote.
10. **Simulated hardware cannot exist in a device build, and CI proves it on every build.**

Engineering invariants alongside these:
- Only the `CardActor` thread talks to the card.
- Only the app process writes `repo/`. Background tasks never touch `window/`.
- No process holds a file lock in the shared container across a suspension point.
- Saving never needs the key or the network.
- Every state reachable on a device is reachable in the simulator through `SimWorld`. A flow that cannot be scripted there is not finished.

Rule 6 excludes remembering the `login` and `url` of opened entries for the AutoFill bar. The bar and row subtitles use path-derived data, plus associations the user made by hand.

### 5.7 Build pipeline

- Pin every GitHub Action to a commit SHA. Today `build.yml` uses tags (`actions/checkout@v4`, `Swatinem/rust-cache@v2`, `upload-artifact@v4`, `download-artifact@v4`).
- Add `permissions: contents: read`. Never use `pull_request_target`. No signing material in any workflow that runs on pull requests.
- Use `cargo build --locked` and `cargo test --locked`; the workflow's `cargo test` has no `--locked` today. Add `cargo deny` as a required job. Pin the XcodeGen version instead of `brew install xcodegen`.
- Device builds come from a commit on `main` with no restored caches and a provenance attestation, which the owner's Linux machine verifies in `scripts/install.sh` before signing and installing.
- Keep the App Store Connect key (`.env`, `*.p8`) off CI with the least role that works.
- CI artifacts of a public repo are public: tests use only the simulated key and obviously fake fixtures.
- Review `ITSAppUsesNonExemptEncryption: false` in `app/project.yml` before any App Store submission.
- `README.md` says `scripts/build-core.sh` is "macOS only"; the script and the workflow say it runs on Linux. Correct whichever is wrong once 3.10 is settled.

---

## 6. Extension points

| Item | Decision | Note |
|---|---|---|
| `KeyTask` / `SheetStage`, `UnlockAttempt` + `SyncOp`, one `KeySheet` with `KeySheetHost` | Build now | Used by decrypt, clone and push |
| `CardReader` / `CardConnection` split, `CardActor`, APDU trace, `KeySession` with held PIN and de-verify | Build now | Replaces stateless `ffi::decrypt` |
| Per-slot touch policy; KDF refusal | Build now | |
| Two-case `Access` driving the disc and Home sections | Build now | Grows a case with the second unlock method |
| Copies: `CopyCache`, `DeviceKeystore`, `SimKeystore`, per-entry deadlines, `boot_id`, per-entry opt-out, single-use copy | Build now | |
| `SshSigner` with `CardAuthKey` and `DeviceSshKey`; `Connector::probe`; `Remote` with separate fetch and push auth; host key pinning with a deliberate reset | Build now | Both signers ship in v1 |
| Queue as commits ahead of origin; replay; automatic keep-both; re-encrypt parking | Build now | |
| Recipient pinning; `Resolution::Missing` makes a folder read-only; certificate import | Build now | Import can slip if the owner's store has one recipient (8.1.3) |
| Line-preserving `EntryDoc`; fixed-policy `generate()` | Build now | |
| `StoreId` in paths, disk layout, keychain tags, AutoFill record ids; versioned `config.json` | Build now | One store in the UI |
| `PawCore` + `AutofillCore`; `autofill::plan`; `ExtensionKeyReach`; hand-off; associations | Build now | Reach value set by the probe |
| Structured `PawError`; `present()` with stable codes | Build now | |
| App Group and keychain groups; Swift 6 mode | Build now, in M1 | |
| `paw-sim` crate, persisted `SimWorld`, scenarios, `SimStrip`, five-layer separation | Build now | Every flow reachable in CI |
| Lock-free cross-process files; Darwin notification; two-process tests | Build now | |
| CI hardening (5.7) | Build now | |
| YubiKey over NFC | Leave a seam | Applet-agnostic transport, neutral stage vocabulary. Also the route to Lightning iPhones |
| Software key; Secure Enclave key as recipient | Leave a seam | Extract the provider trait then; rule 1 must be revised deliberately |
| Recipient editing, sharing | Leave a seam | Keyring already holds imported certificates. Realistically sharing is a second store, since a subfolder exposes every name |
| age / passage | Leave a seam | `pgp` in one file, opaque key handles, store-format names confined to `store/` |
| HTTPS token; device key for push | Leave a seam | Second `Connector`; `push_auth` field |
| Field-level compare of two kept versions | Leave a seam | Both versions exist as entries; a compare view needs both open |
| TOTP | Built | `core/src/otp.rs` (RFC 6238, SHA1/256/512, 6 to 8 digits, pass-otp's `otpauth://totp` line); `OpenedEntry::otp_code(now_ms)` and `otp_problem()`; the editor's "Setup key" takes a base32 key or an address. No QR scanning yet; AutoFill does not offer codes |
| Passkeys | Leave a seam | `FillRequest` enum in the extension model; reach is recorded per request type because the one hang report was for passkey requests |
| Password generator options | Leave a seam | `generate()` takes a policy later |
| Unblock the PIN with the admin PIN on the phone | Leave a seam | See the appendix |
| KDF-enabled cards | Leave a seam | Contained in `card/openpgp.rs` |
| Folder rename and delete | Leave a seam | One change per entry; needs confirmation design |
| Batch operations (re-encrypt a store for a new key) | Leave a seam | One touch per entry bounds how good this can be |
| Commit signing | Leave a seam | Single `commit()` function; a later `KeyPurpose` |
| Onboarding for strangers | Leave a seam | `SetupStep` variants (`CreateStore`, `GenerateKey`); "Check my key" diagnostic; demo data |
| Mac app | Leave a seam | No UIKit in `app/Shared`; expect contention with `gpg-agent`/`scdaemon` |
| iPad split view | Leave a seam | Router and shared views are ready; it follows iPhone |
| Stored PIN behind Face ID | Ignore for now | It turns "key + PIN + touch" into "key + face + touch"; needs its own opt-in |
| Multiple-stores UI, sharing UI | Ignore for now | |
| Health checks, breach lookups | Ignore for now | Touch-per-decrypt limits audits to opened entries; lookups conflict with the privacy stance |
| `.gpg-id` signatures, signed-commit verification | Ignore for now | The real fix for a hostile writer |
| RSA cards | Ignore for now | Needed before a public release; the simulated key covers Curve25519 only |
| `paw://` URL scheme, Shortcuts | Ignore for now | Must not trigger a decrypt |
| Generic wizard engine, plugin registry, `GitBackend`, `CipherFormat`, `Identity` and `StoreFormat` abstractions | Ignore for now | One implementation each |
| Async UniFFI | Ignore for now | State objects cover it |

---

## 7. Build order

Milestones are in dependency order. M0 runs in parallel with M1a.

| # | Milestone | Simulator / Linux with the simulated key | Needs the real device |
|---|---|---|---|
| M0 | **Git and SSH spike (Linux).** The decision rule in 3.10, starting with the russh cross-compile | Entirely; exit test in 3.10 | none |
| M1a | **Foundations and device questions.** App Group and keychain groups in `project.yml`, checked in the simulator job. Split `paw-core` / `paw-sim` with per-slice libraries and bindings. Persisted `SimWorld`. APDU trace in the transaction wrapper. `SimulatedKey` upgrades (empty VERIFY first). CI hardening and device-library scan. A raw-APDU script runner in `DiagnosticsView`, in the app and the extension | Trace classification tests; sim behaviour tests; denylist predicate; container round-trip in the simulator | Run the scripts and export status words and timings: VERIFY empty; VERIFY P1=FF; `endSession` and re-`beginSession` verification state; `endSession` during a touch wait; INTERNAL AUTHENTICATE with chained and extended payloads; `6982` then empty VERIFY after a touch timeout; late session grant; the extension reaching the card on both iOS versions |
| M1b | **Key session and key sheet,** gated on M1a's device answers. Structured errors and `present()`; `CardActor`, `CardSession`, `KeySession`; `UnlockAttempt`, `KeyTask`; `KeySheet` and `KeySheetHost` replace the spike screen | All of 2.11 as Linux state-sequence tests and `KeyFailureTests`; canary test | PIN once per plug-in across lock and background; cancel and unplug during touch; touch-timeout message; no "Save Password?" prompt |
| M2 | **Read a store.** `StoreTree`, `NameIndex`, `EntryDoc`, `Access`; Home, folders, search, entry screen, copy, reveal, Large Type, screen privacy, the 2-minute grace. A fixture store in the simulator | `BrowseTests`, `EntryTests`; GnuPG interop fixtures; property tests | Open entries from the real store, imported as a tarball through a debug-only file importer in `DiagnosticsView` (Files app; no Mac needed); pasteboard expiry after termination; app-switcher cover |
| M3 | **Face ID copies.** `DeviceKeystore`, `CopyCache`, `SimKeystore`, `SimClock`; Lock now; expiry while open; opt-out | `UnlockTests`; all window Linux tests | `SecureEnclaveKeystore`: Face ID per entry, lockout text, enrolment change, passcode removal, reboot ends copies |
| M3b | **Extension skeleton.** `AutofillCore`, `index.bin`, identity store, minimal provider view, fill from a copy; lock-free files, Darwin notification | Two-process Linux tests; allocator cap; no-socket test; `AutoFillTests` for copy fills | The real extension on a device: suggestion bar, Face ID in the extension, memory, staleness after a pull. This proves the container, keychain group and `window/` layout before more is built on them |
| M4 | **Clone, setup, fetch.** `Connector`, `SshConnector`, `probe`, `CardAuthKey`, known hosts; `SyncOp`; `SetupFlow` S0–S6; recipient binding and pinning; fetch and fast-forward with card auth; footer and "Updates need your key" | `SetupTests` against `SimRemote`; in-process russh server on Linux | First real clone from GitHub with the YubiKey; key-check before PIN; host-key screen on a private host. After this the app is usable read-only every day and stays current |
| M5 | **Write and push.** `EntryDraft`, generator, encryption to pinned recipients, certificate import, single-path commits, queue screen, undo, discard; push with fetch, replay and keep-both on one connection | `EditTests`; `pass show` reads paw-written entries; byte-exact edit property test; two-repo replay tests | A real push after a desktop commit: one touch; the desktop `pass` reads the result |
| M6 | **Silent pull.** `DeviceSshKey`, background-updates card with write probe, background refresh, re-encrypt parking, replaced history, recipients-changed prompt, full attention row | `SyncTests`; two-repo Linux tests | Deploy key on GitHub; background refresh while locked; no kill on suspension |
| M7 | **AutoFill, the rest.** Key sheet in the extension (if M1a says the card is reachable), choice screen, reach probe, hand-off with Home banner, single-use copy, associations, setup S6 text | `AutoFillTests` through `-pawHostProvider`; `plan` and matching tables | Key reach on both iOS versions; hand-off round trip; single-use copy |
| M8 | **Settings and finish.** All of 2.12, replace key, change address, diagnostics export, Remove store, quick action | `SettingsTests` | Full manual checklist on iPhone |
| M9 | **iPad.** Split view, form sheets, shortcuts, biometric naming | Same tests on the `iPad` matrix entry | USB-C on iPad; Touch ID wording |

Notes on the order:
- M1a precedes M1b because the device answers decide how `KeySession` holds and clears verification, how cancel works, and whether `CardAuthKey` is possible as designed. The existing self-test screen only reads the key and does a round trip, so it cannot answer them.
- Fetch, fast-forward and replay land with clone and push (M4, M5), not with silent pull. Otherwise the first desktop commit would make every phone push a non-fast-forward with no recovery.
- M3b comes early because the extension dictates the container, the keychain group, `index.bin` and the `window/` layout that M2 and M3 produce.
- M7 can move ahead of M5 and M6 if AutoFill matters more to the owner than editing on the phone. It depends on M3b and M4.

---

## 8. Open questions and risks

### 8.1 Decisions for the owner

| # | Question | Recommended default |
|---|---|---|
| 1 | Face ID copies: each entry lives the chosen length from its own key touch, or one shared window with a fixed end? | **Per entry.** It differs from the literal "until a timed window ends", but avoids copies that vanish minutes after a touch and keeps the rule that no copy outlives the chosen length. If one shared end is preferred, the sheet and footer must show "Face ID until 18:40 (5 minutes)" |
| 2 | How long does a backgrounded paw keep the PIN, and does a device lock clear it? | **15 minutes in the background; a device lock does not clear it.** This honours "once per plug-in" in practice. The stricter alternative (clear at lock, copy changed to "Asked again after your iPhone locks") means typing the PIN on most uses |
| 3 | Does the store's `.gpg-id` have more than one line (a backup key), and is there a second YubiKey with the same subkeys? | Unknown. If more than one line: certificate import stays in M5. If one line: it can slip. A duplicate key works either way |
| 4 | Open Home without Face ID? | **Yes** for the single-user build; turn "Require Face ID to open paw" on by default for a public release |
| 5 | Unsigned commits from the phone? | **Yes.** Card signing costs a touch per push; add it later as a setting |
| 6 | A fixed-policy **Generate** button in v1, although generation is on the later list? | **Yes.** Add is weak without it and the function is pure |
| 7 | PIN in the extension: ask every time, or drop the in-extension key path and always hand off? | **Ask every time, offered beside "Open in paw first".** Decide again after the device spike |
| 8 | Is the single-use hand-off copy acceptable when "Always ask for my key" is chosen? | **Yes.** Without it AutoFill cannot work for those entries when the extension cannot reach the key. It lasts 2 minutes and opens once |
| 9 | Lengths offered for Face ID copies? | 15 min, 1, 4, **8**, 12 hours |
| 10 | Should a newly added entry get a Face ID copy without a key touch? | **Yes**, for the chosen length from the save. If no: "Saved. You'll need your key to open it again." |
| 11 | Bundle and group identifiers (`com.marktoda.paw`, `group.com.marktoda.paw`) for a public repo and App Store? | Keep; the bundle identifier is already in `app/project.yml` |
| 12 | Lightning iPhones (11 to 14 run iOS 26)? | **Not supported in v1.** NFC is the later route |

### 8.2 Technical risks to prove

| # | Risk | Design dependence | Default if it goes badly |
|---|---|---|---|
| 1 | The extension reaching the USB-C key (one report: worked on iOS 26.6, hung on 27.0 for passkey requests) | None structurally: copies, hand-off and the single-use copy cover both outcomes | Treat as `Blocked`; remove the smart-card entitlement from the extension |
| 2 | russh cross-compiling for iOS from Linux; russh external signer and key query; gix transport and pack APIs | Largest schedule risk; all of `sync/` | `git2` with a russh transport; core cross-compile on the Mac runner; tests stay on Linux |
| 3 | De-verifying the PIN over CryptoTokenKit; what `endSession` does to card state; empty VERIFY as a status query | Whether another app could ride paw's PIN; touch-timeout classification | Hold the exclusive session while foregrounded and tell the user to unplug when done; treat `6982` on decipher as a touch timeout |
| 4 | `endSession` interrupting a pending touch wait | How cancel feels | Close the sheet at once; `KeyBusy` until the card's 15 s timeout runs out |
| 5 | INTERNAL AUTHENTICATE payload size over `TKSmartCard.transmit` | SSH auth with the card at all | The simulator supports chaining and extended length; use whichever the device accepts |
| 6 | Two session channels on one authenticated connection at the git host | One touch or two when the server moved | A second connection and touch in that case only |
| 7 | App Group container and shared keychain group in the simulator with `CODE_SIGNING_ALLOWED=NO` | Every XCUITest from M2 on | Ad-hoc sign the simulator build with entitlements |
| 8 | Face ID for a Secure Enclave key inside the credential provider | Copy fills in AutoFill | None; without it AutoFill is hand-off to copy-paste. Found in M3b |
| 9 | A credential provider opening its containing app; a cheap way to run the reach probe in the real extension | Not relied on | Instruction text, as designed |
| 10 | Extension memory with one library | The 120 MB limit | Trim the index; split a library only if measured necessary |
| 11 | Pasteboard expiry after termination; the AutoFill enable prompt API | Copy toast accuracy; setup S6 | Clear on next foreground; **Open Settings** |
| 12 | Read-only probe of the deploy key on hosts other than GitHub | Background-updates self-check | Show "paw couldn't confirm this key is read-only" and continue |
| 13 | Deleting a `WhenPasscodeSet` keychain item while locked; `BGAppRefreshTask` timing | None: background code never touches `window/` | Correctness rests on the read-time check |
| 14 | Reading "cached" touch policies; the library's `touch_required()` reduces them to a boolean | The S1 lines | Read the policy bytes directly |
| 15 | The `ykman openpgp access unblock-pin` and `addcardkey` wording in user-visible strings | Copy | Verify against current tools before shipping |
| 16 | Whether the `openpgp-card` library handles KDF itself | Whether v1 must refuse KDF cards | Refuse, as designed |
| 17 | NFC presence satisfying the touch policy; rPGP external P-256 ECDH | Later features only | None needed now |

---

## Appendix: Considered and rejected

| Proposal | Why it was not adopted |
|---|---|
| Zeroise the held PIN shortly after paw leaves the foreground (at background-task expiry or device lock) and change the copy to "Asked again after you leave paw" | It contradicts the owner's decision of one PIN per plug-in session: with a 6 to 127 character PIN it means typing it on nearly every use. The accurate parts were adopted: the design no longer claims the PIN is gone at device lock, states that a suspended process keeps it in memory until paw next runs, enforces the limit before any card job on return, and puts the stricter rule to the owner (8.1.2) |
| "Unlock with admin PIN" inside the key sheet in v1 | Three wrong admin PINs leave the key recoverable only by a factory reset that destroys the private keys. That is a worse outcome than waiting for a computer, the flow also needs a set-new-PIN step, and it would be the one card flow whose failure cannot be undone. Left as a seam. The other half of the same point (show tries left before any input) was adopted |
| Delay Lock now behind a 5-second **Undo** toast | Lock now is the panic control and must be immediate and irreversible. An accidental tap costs touches, not data. It was moved into its own destructive section of the menu, away from Sync now |
| A second static library for the extension so that no sync code is linked into it | With the simulator umbrella that is four library variants. The rule is enforced structurally instead: no module path from `AutofillCore` to `sync`, plus a no-socket test |
| A user-facing three-way conflict choice (keep mine, keep theirs, keep both) | Both versions are ciphertext, so the choice is a guess between two timestamps and a wrong guess discards a password. Both are kept automatically and the user deletes one after looking |
| One shared window with a fixed end; a sliding end | See 3.8: fixed strands late-opened entries; sliding lets a copy outlive the chosen length |
| An `Identity` trait with a blocking `decrypt` and a prompt callback; a `SoftwareIdentity` for tests | See 4.1: a second owner of the PIN and touch sequence, a blocked card thread, one product implementation. Test fixtures use throwaway public keys |
| Auto-presenting the push sheet after a save when the key is plugged in | Five saves would be five interruptions, and it breaks "nothing presents itself" |
| A `store.lock` flock in the shared container | A process suspended while holding it is killed by iOS. Replaced by rename-based writes, create-exclusive files and per-process logs |
| Remembering `login` and `url` from opened entries to improve AutoFill suggestions | Breaks rule 6. User-made associations give most of the benefit without storing decrypted content |
| A "Share with my other devices" clipboard switch | It turns off `localOnly` and weakens rule 7 for little gain |
| `Access` as `{ FaceId, NeedsKey, ReadOnly }` | Read-only is about writing to a folder and is independent of how an entry is opened. It is `FolderNode.writable` instead |
