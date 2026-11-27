# paw design

Sections 1, 2 and 8 are the product design: the flows as they are meant to be, the decisions put to the owner and the risks. Some of what section 2 describes is not built yet; [roadmap.md](roadmap.md) says what is. Sections 3 to 7 describe the system as built. The code is the authority on names and shapes, so those sections point at it and keep what the code cannot say: reasons, guarantees, and what is still unproven.

Settled since the first draft (2026-11-22):
- Both the app and the AutoFill extension decrypt with the YubiKey over USB-C on an iPhone running iOS 27.2.1 (risk 8.2.1). The smart card entitlement is not needed on iOS.
- Background pull uses a GitHub sign-in (a GitHub App with read-only access to one repository). A deploy key for other hosts comes later. Both are optional.
- A store with no remote is supported. Face ID copies are on by default. See "Decisions so far" in [principles.md](principles.md).

Markers: **UNPROVEN** means the design depends on a fact nobody has verified. **OWNER** means a default only the owner can confirm (collected in 8.1).

---

## 1. Summary

paw is a thin SwiftUI app over one Rust crate, `paw-core`, which owns all state and runs every flow as a state machine. Swift renders what the core returns and implements five traits the core declares: the card connection, the device keystore, the clock, the token vault and the key task listener (3.2).

Every use of the YubiKey (open an entry, clone, push, setup check) is a core object, `KeyTask`, started from a workflow's `KeyJob` on one driver, `KeyTaskDriver` (app/Shared/KeyTaskDriver.swift), and drawn by one sheet, `KeySheet`: plug in, PIN, touch. An entry opened with the key gets a copy sealed under a Secure Enclave key and stays available by Face ID for a fixed length of time measured from that touch (8 hours by default). Each copy has its own deadline.

Saving never needs the key or the network. Edits are single-path local commits, and the "not yet pushed" queue is the commits ahead of the server. Pull without the key uses a read-only GitHub sign-in; a Secure Enclave deploy key for other hosts comes later. Local commits are replayed onto whatever the server has, never merged. When both sides changed one entry, both versions are kept automatically.

The AutoFill extension uses the same core, read-only on the store. It fills from Face ID copies, or with the key in its own key sheet. A hand-off to the app, for an iOS version on which the extension cannot reach the key, is designed (2.10) and not built.

Every hardware dependency has a simulated twin in the core's `sim` module, behind the default `sim` cargo feature, so each flow runs on Linux and in the simulator; a Release device build leaves it out (`--no-default-features`, checked by `scripts/check-no-sim.sh`).

Things that changed relative to the owner's stated decisions, and need confirming:

- **Per-entry deadlines** instead of one shared window end (8.1.1).
- **A background limit on the held PIN** in addition to "once per plug-in" (8.1.2).
- **USB-C iPhones only** in v1. iOS 26 also runs on Lightning iPhones (11 to 14), which cannot take a YubiKey 5C NFC by cable.

---

## 2. User flows

### 2.1 Rules that hold across every flow

1. **The gold disc means "the key is needed" and nothing else.** It appears on a row, button or banner only when the next tap will ask for the YubiKey. It is derived from whether the entry has a live Face ID copy, or from an operation's key requirement, never from "the user owns a YubiKey". No gesture opens the key sheet from a control that did not show the disc.
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

**The key sheet (`KeySheetContent`, drawn for a driver by `KeyTaskSheet`; its words in `app/Shared/Words/KeySheetWords.swift`).** A medium-height sheet (a centred form sheet on iPad). The title comes from the purpose: "Open github.com", "Download your store", "Send 2 changes", "Check your key", "Update your store", "Fill github.com". Cancel is always present.

| Stage (`KeyTaskState`, drawn as `KeySheetStage`) | Headline | Body | Controls |
|---|---|---|---|
| `PlugIn` | Plug in your YubiKey | USB-C, on the bottom of your iPhone. | Cancel |
| `Pin` | Enter your key's PIN | The PIN for the key, not your iPhone passcode. Asked once while the key stays plugged in. | Secure field `pin-field`, "Show" toggle, **Continue** (`pin-continue`, off until 6 characters) |
| `Busy` | One moment | Your key is finishing something else. | Cancel |
| `Touch` | Touch your YubiKey | Touch the gold contact on the key. It's blinking. | Drawing of the key with the contact highlighted and a 15-second ring (`touch-prompt`) |
| `ConfirmHost` (setup only: a screen of its own, not a key sheet stage) | First time connecting to git.example.org | Fingerprint, key type, "Compare this with what your server's admin gave you" | **Trust this server**, Cancel |
| `Working` | From the purpose, e.g. "Downloading your store…" | "412 of 1,240" when known | Cancel |
| `Done` | none | none | Closes itself with a success haptic. No "Success" screen |

- The sheet opens at the first stage that still needs doing. Key in and PIN held means it opens at Touch.
- For an entry, the sheet names the entry before the card starts waiting, because one touch releases one named entry.
- `Pin` shows the tries left before any input whenever fewer than 3 remain: "2 tries left." or "1 try left before the key locks." With 1 left, Show turns on and the button reads **Use last try** from the start. Another computer may already have used tries.
- If paw dropped the held PIN because of the background limit (3.4), the `Pin` body adds: "paw forgot your PIN after 15 minutes away."
- The Touch drawing is the key, not a button. Tapping it shows "Touch the key itself, not the screen." The gold disc glyph alone is never used as the touch prompt.
- `Touch` is shown only once the command that waits for a touch has gone out to the card (3.5). Until then the stage is `Working` or `Busy`.
- For clone and push, `Working` ("Connecting to github.com…") comes before `Touch` as well as after, because the signature is requested on an open connection (3.5).
- If the relevant slot's touch policy is off, the Touch stage is skipped.
- Cancel during Touch closes the sheet at once. A late touch result is thrown away.
- The PIN field is a full keyboard with 6 to 127 ASCII characters and no content type. No PIN field sets a content type (it would invite a "Save Password?" prompt).

**The attention row.** One row at the top of Home shows the most important item, plus "and 2 more" when there are others. Tapping it opens that item, or a list when there are several. Priority: a changed host key, a replaced history, changed recipients, held-back changes, a refused key on push, a refused key on pull, a hand-off request, unsent changes, kept-both notices (informational, dismissible). Offline is not an attention state.

**The sync footer.** One quiet line at the bottom of Home:

| State | Text |
|---|---|
| Device key set up, in sync | "Up to date · checked 2 min ago" |
| Just pulled | "Updated just now · 3 entries changed" |
| Offline | "Offline · last checked 09:12" |
| No device key | Gold disc + "Updates need your key · last checked Mon" (tappable: opens `KeySheet` "Update your store") |
| Sync stopped by the user or an attention item | "Sync paused · Review" (tappable: opens the attention item) |

### 2.4 First-run setup

Seven screens, one job each, Back on all but the first, resumable from the saved step. Background updates are not in setup: adding a deploy key means signing in to the git host, whose password is in the store that is not yet usable. They are offered afterwards as a Home card.

**S0 Welcome.** Title "paw". Body: "Your pass store, opened with your YubiKey." Small print: "You'll need your YubiKey and the SSH address of your password store." Button: **Set up**. On an iPhone without USB-C: "paw needs an iPhone with USB-C for now." and no button. For the App Store, add "Try with demo data" (App Review has no YubiKey). That demo path needs a design of its own, because the simulated key must not exist in device builds (5.5).

**S1 Your key.** Headline "Plug in your YubiKey". It advances when a key appears. Then a card, "YubiKey · serial 12 345 678":
- "Decryption key · ends 2C11 41DE · needs a touch every time"
- "SSH key · ends 77B0 9F3A · needs a touch every time"

No PIN is asked; reading the key needs none. Button: **Continue**. Touch policy is per slot, so each line carries its own. Variants:
- Off: "no touch needed", with the warning "Your key does not ask for a touch for this. Anyone with this iPhone, your key and its PIN could read everything."
- Cached: "one touch covers about 15 seconds", with "During those seconds a second entry can be opened without another touch."
- No authentication key: see 2.11. Setup stops here.
- A PIN KDF setting paw does not know: "This key uses a PIN setting (KDF) that paw doesn't support yet." Setup stops. No PIN try is spent (3.4).

paw still works with weaker touch policies. The policy is the user's.

**S2 Store address.** Headline "Where is your password store?" Field `repo-address`, placeholder `git@github.com:you/password-store.git`, Paste button. Button: **Connect**. Inline validation:
- `https://…`: "paw connects over SSH for now. Use the address that starts with git@ or ssh://."
- unparseable: "That doesn't look like a git address."

Connect runs a key check before any PIN or touch. paw connects, verifies the host key, and asks the server whether it would accept the card's SSH public key (an SSH public-key query, which carries no signature). Result line:
- "github.com accepts this key." Setup moves to S3.
- "github.com doesn't know this key yet." Followed by "Your key's SSH key", one truncated line `ssh-ed25519 AAAA…`, and three ways to add it:
  - **Copy** and **Share…** (share sheet), plus **Show QR code** for a computer's camera.
  - "On your computer:" `gh ssh-key add` with the key inline, selectable.
  - **Open GitHub** (deep link to `https://github.com/settings/ssh/new`) when the host is github.com.
  - Then **Check again**.

A server that accepts the key but not the repository is caught at S3. russh sends the query before any signature (3.10).

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

**S6 AutoFill.** Headline "Fill passwords in Safari and apps". With the key still plugged in, paw first runs the reach probe (not built, 3.11): "Checking whether your key works inside Safari…". The body then states what the user will get:
- Probe says the key works in the extension: "Passwords you opened recently fill with Face ID. Others ask for your key's PIN and a touch, or you can open them in paw first."
- Probe says it does not, or could not tell: "Passwords you opened recently fill with Face ID. For others, paw will ask you to open paw and touch your key, then come back."

Button **Turn on AutoFill**. On return: "AutoFill is on." or "AutoFill is still off. You can turn it on in Settings › General › AutoFill & Passwords." with **Open Settings**. "Not now" is allowed. **UNPROVEN:** the system prompt API for enabling a credential provider on iOS 26; **Open Settings** is the fallback.

**S7 Done.** No screen; Home appears with the card below.

**Home card: "Turn on background updates".** Body: "This iPhone can check for changes on its own with a read-only key that never leaves it. It can't change your store." Opening it shows:
1. "Copy this iPhone's key" (**Copy**, **Share…**, **Show QR code**; suggested label "paw on iPhone, read-only").
2. "Add it to your repository as a read-only deploy key": **Open GitHub** (deep link to `https://github.com/<owner>/<repo>/settings/keys/new`) when the host is github.com; "On your computer:" the `gh repo deploy-key add` line; otherwise instructions.
3. **Check**.

Results: "Background updates are on." / "The server doesn't accept this iPhone's key yet." **Check again** / "This key can write to the repository. Add it as read-only." Dismissing the card: "Not now. paw will only update when you plug in your key and ask." The card can be reopened from Settings › Store.

**As built.** Setup is a state machine in the core (`core/src/setup/mod.rs`, `SetupState`), saved after every step, with nothing secret in the file.
- The key steps run as key tasks on the session (`Setup.read_key_from_session`, `download_with_key`, `test_decrypt_with_key`), so a whole setup asks for the PIN once. Checking the host and confirming its key need no key. A cancelled or dropped download leaves nothing on disk, and the next try needs no PIN.
- When the test decrypt succeeds, setup pins the card's key, binds its `.gpg-id` line (3.7), accepts every `.gpg-id`, and records the key as the store's enrolled key (5.6). `Setup.write_readiness` says whether saving works with no further step, and otherwise which recipients paw has no key for; setup is done either way.
- Addresses (`core/src/sync/address.rs`): `owner/repo`, `github.com/owner/repo` and `https://github.com/owner/repo` become `git@github.com:owner/repo.git` with a notice. Refused: a password, token or user name in the address (never repeated back), git's other transports, and anything starting with `-`.
- Bundled host keys are github.com (and `ssh.github.com:443`), gitlab.com and bitbucket.org. codeberg.org and git.sr.ht publish fingerprints but not keys, so they are confirmed on first use, unlike S3's list above. A bundled host's key cannot be forgotten.
- An empty repository, or one with no `.gpg-id`, sends setup back to the address step with the reason; S3's "Continue anyway" is not built. A host that knows the key but not the repository is told apart from a key it does not accept (`RemoteFailure`).
- When github.com cannot be connected to at all on port 22, setup offers `ssh.github.com:443` and switches only when asked (`Setup.use_port_fallback`).
- A key whose PIN KDF paw cannot follow is refused at "Your key" (`UnusableKey`); the usual KDF settings are supported (3.4).
- The repository can also be picked from a list after a GitHub sign-in, which gives the same SSH address (5.3).

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
- An entry changed by a pull loses its copy. An edit made while a copy is live re-seals the new version with the same deadline. A rename made in paw keeps the copy (it is re-sealed under the new name, 3.8).
- A new entry saved with Face ID copies on gets a copy for the chosen length from the save. The user just typed that password on this phone, usually to fill it straight into a sign-up page. **OWNER** (8.1.10).
- Per-entry opt-out: an entry screen toggle "Always ask for my key" (stored locally by path). Such entries are never given a lasting copy. The entry menu also has "Forget Face ID copy".

**Expiry while the app is open.**
- The row moves from "Open with Face ID" to "Recent, needs your key".
- An open entry that was opened from its copy blanks its fields: "This entry's Face ID time ended at 18:40." with **Open with your key** (disc).
- An open editor keeps its draft in memory until closed, because saving needs no key.

**Leaving and coming back.** An open entry survives leaving the app for 2 minutes, so "copy username, switch, paste, come back, copy password" costs one touch or one Face ID. On return within 2 minutes the entry is masked but copyable. After 2 minutes away, a device lock, or Lock now, the entry screen shows "Closed while you were away." with the usual open button. A suspended process runs no code, so the plaintext is discarded when paw next runs, before anything is drawn (5.2 states this honestly).

**Lock now.** One tap, no confirmation and no undo: it is the panic control, and an accidental tap costs touches, not data. It wipes every copy, forgets the PIN, de-verifies the card if present, closes open entries, and clears the clipboard if paw filled it. Toast: "Locked. 6 entries now need your key." Also a Home Screen quick action.

### 2.6 Browse and search

- **Folders** are grouped rows with a tinted icon chip, entry count and chevron. The colour is a stable hash of the top-level folder name into the Keyring 2 palette.
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

**As built** (`core/src/write/`, `core/src/generate.rs`; `StoreWriter`, `EntryDraft`). Each change is one commit of one entry in pass's wording, always authored "paw <paw@paw.invalid>" (there is no per-store git identity). Differences from the above:
- Moving to a folder with other recipients is built: it re-encrypts, as `pass mv` does, and so is an edit of the open entry whose folder changed. There is no move or rename without the entry open. It is one commit worded "Rename …".
- An edit that also renames is two commits, "Rename …" then "Edit …".
- An edit, a rename or a move re-seals the entry's Face ID copy for the time it had left. A new entry gets a copy for the usual length.
- Who paw may encrypt to, and how a `.gpg-id` change is accepted, is in 3.7. A folder whose list changed refuses saving with `RecipientsChanged` (old and new lines) until it is accepted.
- Names are checked for collisions ignoring letter case and Unicode normalisation, as APFS does.
- The generator takes a policy; the button uses the fixed default (20 characters, all four classes, a conservative symbol set, look-alikes left out). There is no word mode: no word list with a clear licence is embedded yet.

### 2.9 Sync

**Silent pull.** Runs on foreground, pull to refresh and system background refresh, with the device key. No UI except the footer. With no local changes it fast-forwards. With local changes the core replays them onto the server's tip, without the key. paw's commits are unsigned. **OWNER.**

**Push.** `KeySheet` titled "Send 2 changes": PIN if needed, "Connecting…", touch, "Sending…", done. One push is one SSH connection and one touch. The fetch, any replay and the send all run on that connection, including one retry when the server moved. The intent is to check reachability and whether the server accepts the key before any PIN or touch; as built, a send verifies the PIN first, and a key the server refuses then costs no touch (3.5). **UNPROVEN:** that the git hosts allow an upload-pack and a receive-pack session channel on one authenticated connection (standard SSH; OpenSSH connection sharing works against GitHub). Fallback: a second connection and a second touch, only when the server moved.

**Offline.** Everything local works. Footer: "Offline · last checked 09:12". A push attempt shows, before any PIN or touch: "You're offline. Your changes are saved on this iPhone and will wait."

**Both sides changed the same entry.** Each entry is one opaque encrypted file, so the only conflict is the same path changed in both places. Both versions are ciphertext, so the user cannot choose between them blind. paw therefore never asks; it keeps both.
- Edited on both sides: the server's version keeps the name. The iPhone's version is saved beside it as "github.com (from this iPhone 2026-11-23)", dated by the local change (a number is appended if that exists). Row: "Two versions of github.com were kept." The detail lists both with **Open** on each (disc where needed) so the user can compare and delete one.
- Edited here, deleted there: the iPhone's version is kept under its own name. Row: "github.com was deleted on laptop but changed here. Your version was kept."
- Deleted here, edited there: the server's version stays and the local delete is dropped. Row: "github.com was changed on laptop, so it wasn't deleted."
- Deleted on both sides: nothing to do, no row.

These rows are informational and dismissible. Sync is never blocked and nothing is lost. "Changed on laptop" comes from the commit author name.

**History replaced** (the server's head is not a descendant of the last one paw saw): sync stops. "The store on the server was replaced." / "Its history no longer matches this iPhone's. Nothing here has been changed." Buttons:
- **Use the server's store and keep my 2 changes** (default when a queue exists; the changes are replayed onto the new history with the rules above). With no queue: **Use the server's store**.
- **Use the server's store and discard my changes** (confirm "Your 2 unpushed changes will be lost.").
- **Not now**. The footer then reads "Sync paused · Review" and the attention row keeps the item, with any other items behind "and 2 more".

**Recipients changed by a pull.** Attention row: "Who can open work/ changed." Detail: "Changed on laptop, 3 Nov. Before: 1 key (yours). Now: 2 keys (yours, and one ending 9A1C 03F2)." with fingerprints under Details, and **Accept** / **Not now**. Until accepted, paw does not write in that subtree (5.3). After **Accept**, if paw lacks a public key for a new recipient, the next screen is "Add the missing key" (2.12); until that is done the folder stays read-only and says so.

**A queued change is encrypted for the old recipients.** If a pull changes the governing `.gpg-id` of a path with a queued change, that change is held back and nothing is sent while it is (3.6). Sync sheet row: "1 change wasn't sent", "Who can open the folder changed after you saved." **Review** opens the held-back screen. Once the new list is accepted, **Re-encrypt 1 change** (disc) sits in the sync sheet's bottom actions (`sync-reencrypt`) and on the held-back screen (5.3, `held-back`).

**Auth failures.**

| Case | Where | Copy |
|---|---|---|
| Device key refused | Attention row | "Background updates stopped." / "The server no longer accepts this iPhone's key." **Fix** opens the background-updates screen |
| YubiKey refused on push | Key sheet, before PIN or touch | "The server didn't accept this key." / "Your changes are still waiting on this iPhone." **Copy SSH key**, **Try again** |
| Push refused by a server rule | Key sheet | "The server refused the changes." / server message under Details |
| Host key changed | Attention row or sheet | "The server's identity changed." / "paw stopped syncing to be safe. If the server's admin confirms the change, go to Settings › Store › Forget server key." Old and new fingerprints |

The only way past a host key change is Settings › Store › "Forget server key", a separate deliberate act. A bundled host's key cannot be forgotten; a change there waits for an app update (5.3).

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

### 2.11 Hardware failures

All inline in `KeySheet` unless noted.

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
| Unknown KDF setting | Reading the key | "paw doesn't support this key's PIN setting yet." | "The key has a KDF setting paw doesn't know. No PIN try was used." |
| No OpenPGP applet | Reading the key | "paw can't find OpenPGP on this key." | "It may be turned off for USB, or this may not be a YubiKey." |
| Key busy | Session not granted in 10 s | "Another app is using the key." | "Unplug it and plug it back in." |
| No smart card support | Opening the connection | "This iPhone can't talk to USB keys." | Setup only |
| Entry not readable | Parsing | "'github.com' isn't a readable entry." | "The file may be damaged. Other entries are not affected." |
| Hidden recipient, decrypt fails | After touch | "This key couldn't open 'github.com'." | The wrong-key pre-check cannot run when key IDs are hidden (`throw-keyids`) |

A second YubiKey that holds the same subkeys is not a wrong key. Keys are matched on the decryption fingerprint; the serial is a label (3.4).

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
- CI runs the UI tests on an iPhone simulator and `IPadTests` on an iPad simulator, as a part of its own.

---

## 3. Architecture

### 3.1 Crates and modules

One Rust crate, `paw-core` (`core/`), exported to Swift with UniFFI. An exported type or function is declared in the module it belongs to; there is no separate export layer. Each module's own `//!` comment says what it is for and what it must not do; this list only says where things are.

```
core/src/
  key/              one connected card: session, held PIN, card jobs, touch watch, key log, enrolled key
  task/             key tasks: every workflow that needs the key, and the stages its sheet shows
  copies/           Face ID copies: sealed format, folder, keystore seam
  store/            reading the store: tree, search, site matching, .gpg-id, pairings, one-time-code marks
  write/            drafts, pinned keys, commits, held-back changes, who can open what
  sync/             git over SSH: address, known hosts, transport, tree guard, integrate, queue
  setup/            first-run setup as a state machine
  github/           GitHub sign-in, repository list, HTTPS fetch, background refresh
  files/            the state directory's file names and formats, and the refs paw keeps
  entry.rs, entry/  an entry's text, and an opened entry
  open.rs           opening an entry with the key or from its Face ID copy
  lock.rs           the store lock
  fault.rs, remote.rs   what can go wrong under any workflow, and with the remote
  git_transport.rs  what lets libgit2 use paw's SSH and HTTPS connections; the download cap
  clock.rs, fingerprint.rs, path.rs, recipients.rs, ssh.rs, otp.rs, generate.rs, report.rs
  sim.rs, sim/      cargo feature `sim`: simulated key, keystore, clock, git host, GitHub, fixture store
core/tests/         one file per area, run on Linux
app/
  Shared/           compiled into the app and the AutoFill extension
  Paw/              the app; Debug/, Gallery/ and Simulator/ are debug builds only
  PawAutoFill/      the extension's entry point
  Generated/        UniFFI bindings (generated by scripts/build-core.sh, not in git)
  PawTests/ PawUITests/
scripts/            build, CI, install, checks
```

Rules the layout keeps:
- `store` works from names and the unencrypted start of each file. It never decrypts and needs neither the key nor the network.
- `key` knows no workflow. `task` knows every workflow that runs as a key task: what it is for, what it produces, how the work after the key fails.
- `files` sits below everything that uses it. On-disk names are declared once, in `core/src/files/layout.rs`; Swift asks the core for them.
- A function that works on a store is given a `StoreDirs` (state directory, the repository at `repo/` in it, and its lock), built from the state directory alone. `OpenStore` (`core/src/sync/mod.rs`) is that repository opened for one operation with paw's git settings applied.
- The `sim` feature is on by default and off in the device library (5.5).

`scripts/build-core.sh` builds two libraries (device without `sim`, simulator with it) and one set of Swift bindings for each; `scripts/make-xcframework.sh` packs them as the two slices of `app/build/PawCore.xcframework`. The iOS libraries need the Apple SDK (the core contains C: libgit2 and ring), so they are built on macOS; on Linux the script makes the host builds and both bindings.

### 3.2 Platform seams

Swift implements five traits the core declares. Everything else Swift does for the core is a thin OS call fed with data the core returns (pasteboard, `ASCredentialIdentityStore`, background task scheduling).

| Trait | Declared in | On a device | In the simulator and on Linux |
|---|---|---|---|
| `CardConnection`: exchange one raw APDU | `core/src/key/card.rs` | `UsbCardConnection` over `TKSmartCard` | `SimulatedKey` (Rust), behind `SimulatedCardConnection` |
| `DeviceKeystore`: `seal`, `unseal` (Face ID), `index_key`, `reset` | `core/src/copies/seams.rs` | `EnclaveKeystore` | `SimKeystore` |
| `Clock`: `wall_ms`, `monotonic_ms`, `boot_id` | `core/src/clock.rs` | `SystemClock` | `SimClock` |
| `TokenVault`: store, load, remove one named secret | `core/src/github/session.rs` | `KeychainTokenVault` | `SimTokenVault` |
| `KeyTaskListener`: told each stage change of a key task | `core/src/task/mod.rs` | `MainQueueTaskListener` | the same |

The network is not a seam. SSH is russh inside the core (`core/src/sync/transport.rs`) and HTTPS to GitHub is the core's own client (`core/src/github/http.rs`). In tests and the simulator the other end is real protocol on a loopback port: `SimRemote`, an SSH server serving one bare repository with upload-pack and receive-pack written on libgit2 (it starts no `git` program, so it runs inside the app in the iOS simulator), and `SimGitHub`, an HTTP server with the parts of GitHub paw uses.

Notes:
- **The hardware is one value,** `Platform` (`app/Shared/Platform/Platform.swift`): where the key comes from (a `KeyConnector`), the clock, the Face ID keystore, the iPhone's lock as the store's files see it, and the process's name in the key log. It has two constructors, `Platform.device` and, in debug simulator builds, `Platform.simulated`. Each process entry point picks one and hands it down: the app's launch (`LaunchRoot` in `app/Paw/PawApp.swift`), the extension's request (`StoreForAutoFill`), the fixture and the AutoFill harness. Nothing below an entry point picks.
- **Presence.** `KeyReader` follows the platform's `KeyConnector` (on a device, KVO on `TKSmartCardSlotManager.slotNames` and the first reader's card state) and reads a key that is in into the `KeyPort`'s session, with no PIN and no touch. It also tells the session when paw goes to the background and comes back. The reader watch is minimal and unverified on hardware.
- **Late session grant (open risk).** `UsbCardConnection` gives up on `beginSession` after 10 seconds, but the request stays queued with the system. A grant that arrives later gives an exclusive session to an object nobody holds, which would show as "Another app is using the key" against paw itself. Nothing ends such a session yet.
- **Simulation controls.** Debug launch arguments are declared once each, in two lists: `KeyLaunchArgument` (`app/Shared/LaunchArguments.swift`, read by both processes) and `LaunchArgument` (`app/Paw/AppLaunchArguments.swift`). `scripts/launch-arguments.sh` checks that every argument a UI test passes is declared and every declared one is read. A Release build reads no launch argument at all. The simulated key, clock and git host are one per launch (`LaunchedHardware`).
- **Stand-ins for the hardware on screen** are drawn by `SimulatorStrip`, a dashed strip under a screen (the key sheet's touch and plug-in buttons, setup's, and Settings › Simulator). They act on the key the platform handed down, reached through the screen's own `KeyReader`. The simulated git host observes the models (`SimRemoteWatch`, `SimSetupHostWatch`) through a `SimRemoteHandle`; the models never call it.
- Simulator-only Swift lives in `app/Shared/Simulator` and `app/Paw/Simulator`, under `#if DEBUG && targetEnvironment(simulator)`.

### 3.3 Key types

| Type | Where | What it is |
|---|---|---|
| `PassStore` | `core/src/store/mod.rs` | The store's names, read once: folders, entries, search, site matching, who an entry is for. An immutable snapshot; the app opens a new one after a sync or an edit (`LiveStore`). Paths cross the boundary as strings and are checked on every read (`core/src/path.rs`) |
| `Access` | `core/src/recipients.rs` | Whether the plugged-in key can open an entry (`CanOpen`, `MightOpen` for hidden recipients, `NotForThisKey`, `Unreadable`), read from the file's clear-text recipient packets. It is not the Face ID state; that is `CopyStatus` from the `CopyCache` |
| `EntryDoc` | `core/src/entry.rs` | An entry's text, line by line. The contract is that a document parsed and written back is the same bytes, and an edit rewrites only the lines it touches |
| `OpenedEntry` | `core/src/entry/opened.rs` | The long-lived plaintext, held in Rust. Swift gets a view with no secret in it (`EntryView`) and asks for one value at a time |
| `EntryDraft`, `StoreWriter` | `core/src/write/` | The add and edit form, and saving, deleting, renaming and moving (2.8) |
| `KeyPort`, `KeySession`, `KeyTask` | `core/src/key/`, `core/src/task/` | 3.4 and 3.5 |
| `CopyCache` | `core/src/copies/mod.rs` | 3.8 |
| `StoreSync`, `Setup`, `GitHubAccount` | `core/src/sync/store_sync.rs`, `core/src/setup/mod.rs`, `core/src/github/account.rs` | Sync, first-run setup and the GitHub sign-in as Swift drives them |

There is no root object and no observer from the core. Swift builds what a store needs in `StoreSession`, and the one signal that the store changed is Swift's (`LiveStore`). The only callback from the core that reports progress is `KeyTaskListener`. Snapshots carry times, not countdowns: Swift draws "until 18:40" and the code countdown from a deadline.

**What "secrets stay in Rust" means.** Rust holds the only long-lived copy, in types that wipe their memory when dropped, have no `Display` and print nothing in `Debug`. Swift necessarily receives short-lived copies that cannot be wiped: the PIN from the secure field, revealed text, editor field contents, the credential handed to iOS, and pasteboard data. The design keeps those few and late:
- the PIN crosses as bytes (`KeyTask.supply_pin`) and the field is cleared on Continue;
- a secret value crosses only on an explicit call for that one value, when the person taps copy, reveal or fill.

No claim is made that Swift-side copies are wiped. What the `pgp` library copies internally while encrypting is also outside paw's reach.

### 3.4 Card and key session

The app keeps one `KeyPort` for its life. Plugging a key in makes a `KeySession` (`core/src/key/session.rs`); unplugging ends it. The session reads the key's identity, both slots' touch policies and the PIN KDF setting when it is made, with no PIN and no touch (`CardInfo`, `KeyTouch` in `core/src/key/card.rs`). It keeps the reader's exclusive session for as long as the key stays in.

1. **One card job at a time.** A card job is a short run of commands with the card to itself: select the application, make sure the PIN is verified, then decrypt or sign. PIN entry happens between jobs, so a person typing never holds the card. A task that wants the card while another job runs waits, showing `Busy`. That includes a job whose task was cancelled while its command waits for a touch, because nothing can interrupt a command the card is holding.
2. **PIN once per plug-in.** The session holds the verified PIN in memory that is wiped when dropped. It is never written to disk, the keychain or a log, and never shared with the extension, which has its own port and asks each time. Before a job the session asks the card with an empty VERIFY whether it is still verified (no try used) and re-verifies silently if not. If paw holds no PIN it asks for one even when the card says it is verified.
3. **The held PIN is never retried.** If the card refuses it, it is dropped (one try was used finding out) and the person is asked. With zero tries left no VERIFY is sent.
4. **The PIN is dropped** on removal, on any reader failure (which ends the session), on a refusal, on Lock now (`KeySession.forget_pin`), and when paw comes back after `PIN_BACKGROUND_LIMIT` (15 minutes) in the background. When paw forgets a PIN while the key is still in, it also de-verifies the card (VERIFY with P1 `FF`) on another thread, after any command that is waiting for a touch.
5. **Enrolled key.** Setup records the key's identity in the state directory once the test decrypt has shown that it opens the store (`core/src/key/enrolment.rs`). A port given that record refuses every task on another card before any VERIFY (`KeyError::DifferentKey`, naming both keys). Keys match by decryption fingerprint, and by authentication fingerprint where both are known. The serial is recorded and shown but not compared: a second YubiKey loaded with the same keys is the same key. Its PIN is asked anyway, because a held PIN belongs to one plugged-in card. Replacing the enrolled key has no screen yet.
6. **KDF.** A card with the PIN KDF on expects VERIFY to carry an iterated and salted hash of the PIN, and counts the raw PIN as a wrong try. The session reads the KDF data object and sends what the card expects (`core/src/key/kdf.rs`). A setting paw does not know is refused before any PIN is asked, with no VERIFY sent.

**Why the PIN survives a device lock.** The owner's decision is "once per plug-in session". Phones auto-lock within a minute or two, so dropping at lock would mean typing a long PIN on almost every use. Unlocking the phone already needs the passcode or Face ID, and the card still demands a touch per decrypt. The cost is stated in 5.2: someone who can unlock the phone, with the key still plugged in and paw's process alive, needs no PIN.

**What cannot be promised.** A suspended process runs no code. The background limit is therefore applied when paw next runs, before any card job, and that makes the PIN unusable after the limit. The bytes may remain in the suspended process's memory until then. **OWNER** (8.1.2): the 15-minute limit, or a stricter rule.

**UNPROVEN:** whether a YubiKey honours VERIFY P1 `FF` over the iPhone's reader. paw does not rely on it for itself (it asks for the PIN whenever it holds none); what rests on it is that another app cannot ride a PIN paw has forgotten while the key stays in. The empty VERIFY as a status query is answered: the card reports its PIN state (roadmap, "Proven so far").

### 3.5 State machine: key tasks

Every use of the key is one `KeyTask` (`core/src/task/`): open an entry, sign a git login (clone, fetch, push), check the key during setup, re-encrypt held-back changes, the touch timeout check (`KeyPurpose`). The key sheet renders the task's `KeyTaskState` and nothing else; it never combines states or decides anything. The stages are `PlugIn`, `Pin`, `Busy`, `Touch`, `Working`, then one of `Done`, `Failed` (the key), `SyncFailed` or `SetupFailed` (the work after the key), `Cancelled`. There is no stage for reading the card or verifying the PIN (both show as `Working`), and none for a touch timeout: it fails the task with the PIN kept, so "Try again" is a new task that opens at `Touch`.

- **Starting.** A task started with no key connected waits at `PlugIn` and goes on by itself when one is read. On a key, every task starts through `KeySession::preflight`, which refuses, before any PIN or touch: a card that is not the enrolled key, what the task itself cannot do with this key (no decryption key, no Ed25519 authentication key, an entry that names other keys), a KDF paw cannot follow and a blocked PIN. A PIN try is never spent on a key that cannot do what was asked.
- **Threads.** Each task runs on its own thread, which waits for the key, the PIN and the card; none of those waits holds the card. The sheet drives it with `supply_pin` and `cancel`.
- **When `Touch` shows.** Only once the command that waits for the touch has gone out to the card (the session's transport wrapper, `core/src/key/wire.rs`, sees every command), so a touch cannot release some other, perhaps cancelled, command. For a slot whose policy is off, never. For a cached or unknown policy, only if the card is still waiting 400 ms later, because it may answer at once from an earlier touch and a prompt that flashes teaches people to ignore it.
- **Cancel** takes effect at once: the state is `Cancelled` before `cancel` returns. A command already waiting for a touch cannot be interrupted; the task's thread waits for the card's answer, throws it away and only then frees the card. The one exception is the last milliseconds of an open, after the card has answered and while the entry is handed over: a cancel then is too late and the task finishes.
- **One touch releases one named entry.** An open asks the card to unwrap at most one recipient packet: the first that names the card's key, or the first hidden one if none does (`core/src/key/decrypt.rs`). A crafted entry with many packets cannot cost touch after touch. An entry with only hidden packets for several recipients opens only if this key's comes first.
- **A git login** verifies the PIN first, then opens the connection and hands the network work an `SshSigner` that signs through the session (4.2). No PIN can be asked while the server waits: if paw no longer holds one, the signature fails. The work after the login (clone, fetch, integrate, push) runs in the task's thread, reports progress as `Working` and stops between network steps once cancelled.
- With a cached touch policy one touch can cover a second command within about 15 seconds. paw cannot prevent that; setup says so, and rule 2 of 5.6 is weakened for such keys.

**Setup on the session.** Setup's key steps are key tasks (`Setup.read_key_from_session`, `download_with_key`, `test_decrypt_with_key`), so a whole setup asks for the PIN once. Checking the host and confirming its key need no key and are plain calls; the host is asked whether it accepts the card's SSH key before any PIN (`Connection::probe`). A cancelled download leaves nothing on disk.

On the Swift side one `KeyTaskDriver` follows one sheet's tasks (3.12).

### 3.6 State machine: sync

Sync is something the app asks the core to do, not a running engine with a status. A send or a refresh with the key is a key task (`StoreSync.send_with_key`, `refresh_with_key`); a refresh with the GitHub sign-in is a call (`GitHubAccount.refresh_store`). Each ends in a `SyncReport` (`core/src/sync/operation.rs`): up to date, changes still waiting, or the remote's history was replaced. What the pill and the sync sheet show from that is Swift's (`SyncModel`).

- **One login per sync.** A send fetches, integrates and pushes on one SSH connection, so it costs one signature and one touch. Each git service is a channel on that connection. If the remote moves between the fetch and the push, the push is refused (paw never forces); the send then fetches, integrates and pushes once more on the same connection. A second refusal ends the sync (`RemoteFailure::KeptMoving`).
- **Replay instead of merge** (`core/src/sync/integrate.rs`, `history.rs`). The local branch is always the remote's tip plus paw's own commits, one entry per commit. When the remote has moved, each local commit is re-applied, oldest first, on the new tip, with its message and time. Blobs are ciphertext, so nothing is decrypted and there is no three-way merge. Everything is built as new objects first; the branch, index and working copy move last, together, or not at all.
- **Same entry changed on both sides.** The four cases of 2.9. The local version is kept beside the remote's as "`<name> (from this iPhone <date>)`", dated by the local commit and numbered if that name is taken. No user decision.
- **Replaced history.** The tip paw last accepted is a ref in the repository (`core/src/files/refs.rs`). A remote tip that does not descend from it stops integrate until the person decides (`accept_replaced_remote`, keeping or discarding the waiting changes).
- **Not paw's history.** Local commits that are not one-entry paw commits, or uncommitted files in the working copy, stop the sync and change nothing (`SyncError::NotPawHistory`, `UncommittedChanges`). Another tool used this copy; paw does not guess.
- **Held-back changes** (`core/src/write/held_back.rs`). A waiting change whose ciphertext is not for exactly the keys its folder's `.gpg-id` resolves to now is held back, and a send pushes none of the queue while any is. Accepting the new list does not release it: the old ciphertext would still go into history. It is re-encrypted by hand, a key task that asks the PIN once and one touch per change and rewrites the queue as it goes, so a cancel part-way keeps those done.
- **The queue is the commits ahead of the remote** (`core/src/sync/queue.rs`). There is no second record to drift. Only commits after the remote-tracking tip are ever rewritten (undo, discard, replay, re-encrypt); pushed history is never touched.
- **Background.** A `BGAppRefreshTask` calls `GitHubAccount.background_refresh_stoppable`: a fetch over HTTPS with the read-only token, with no key, PIN or Face ID. It integrates only what needs no decision (fast-forward, replay) and leaves the rest for the person, changing nothing: a refused tree, a `.gpg-id` change, a replaced history, held-back changes. It stops between whole steps at its deadline and is safe to kill at any point. It is skipped while the iPhone is locked, because the store's files are unreadable then. Without a GitHub sign-in there is no background pull; a deploy key for other hosts is later (roadmap M6).

### 3.7 Recipients and encryption

- `.gpg-id` names recipients by fingerprint, key ID or e-mail address, but encrypting needs public keys, and the repository is not a trusted place to get them. The keys paw encrypts to are pinned in the state directory, outside the repository (`core/src/write/keyring.rs`). The only way in today is the card's own decryption key, pinned at setup.
- **Binding.** The owner's `.gpg-id` usually names the primary key, while the card exposes only its encryption subkey, and nothing on the phone can prove the two belong together except the person and a decrypt. So setup binds empirically: after the test decrypt it binds the card to the single unsatisfied line of the `.gpg-id` governing the entry it opened, and only if that entry's recipient packets name the card's key. A line is otherwise satisfied only by exactly one pinned key that it names by full fingerprint or long key ID. Short IDs, names and e-mail addresses need a binding.
- **All or nothing.** If any line of a folder's `.gpg-id` is not satisfied, saving there is refused with every such line named. paw never encrypts to fewer keys than `.gpg-id` names. Reading still works. A store with a second recipient (a backup key) can therefore be read but not written from the phone: importing another recipient's public key is not built (roadmap, "After the first version").
- **Changes are never accepted implicitly.** Setup records every `.gpg-id` with its contents. A folder whose governing `.gpg-id` is now another file, or has other contents, refuses saving until the person accepts it for that folder. The record is in the state directory, which a pull never touches.
- New entries are SEIPD v1 with AES-256. Reading is capped at 1 MiB of plaintext. ASCII-armoured entries are reported as unreadable.
- Names are checked for collisions ignoring letter case and Unicode normalisation, as APFS does.

### 3.8 Face ID copies

`CopyCache` (`core/src/copies/`) keeps an entry opened with the key readable by Face ID until that entry's own deadline. The rules are enforced whenever a copy is read, not by a timer that might not run.

- **Format** (`core/src/copies/format.rs`). Each copy has a random data key and is sealed in Rust with XChaCha20-Poly1305. The data key is the only thing the Secure Enclave seals (`DeviceKeystore::seal`), and it reaches Rust again only through Face ID. The copy's metadata (entry path, content id, times, boot id) is the associated data, so a text can never be opened as another entry's or with a deadline it was not sealed with. The metadata itself is sealed under a key derived from the keystore's index key, a keychain secret usable without a prompt, so every deadline is read and checked before any Face ID prompt and the files say nothing to someone who has only them. File names are a keyed hash of the entry path. Plaintexts are padded to a bucket size.
- **Keyed by path plus content id.** The content id (SHA-256 of the `.gpg` file) is sealed inside and compared with the entry's current file on every read. A pull that changes the entry makes its copy stale. An edit, rename or move made in paw re-seals the copy for the time it had left.
- **A copy ends** at its deadline, or once its length has passed on the monotonic clock, whichever comes first. Face ID never extends it. If the wall clock is behind when any copy was made, or the phone has rebooted since (`Clock::boot_id`), every copy ends and the keystore is reset first, which makes every copy unopenable in one step. A copy that does not authenticate is wiped.
- **Every call sweeps.** Each operation first reads every copy's metadata, applies these rules and wipes what has ended. So an ended copy is removed the next time either process touches the cache. Between a deadline and that moment the file exists; every read path fails closed.
- **Open** raises Face ID with a fresh context each time: one match releases one entry. There is no passcode fallback.
- **Length.** 8 hours from the key touch (`FaceIdOpener.windowSecs`), at most 12. A new entry saved in paw gets a copy for the same length from the save (8.1.10). The switch in Settings turns copies off and wipes them. A per-entry opt-out and single-use copies are not built.
- **Key gone.** An enrolment change and a removed passcode both arrive as `KeystoreError::KeyGone`, and every copy is wiped. Each process keeps its own record of the enrolled faces, because iOS gives each bundle a different enrolment hash for the same faces.
- **Invariant.** No copy outlives the configured length measured from the event that put its plaintext on this phone: a key touch, or the person typing a new entry.

Decisions inside this section:
- *Per-entry deadlines* won over one shared end time. A shared, fixed end makes an entry opened five minutes before the end vanish five minutes after a touch, and a sliding shared end lets a copy made at hour 0 live to hour 15. Per-entry deadlines avoid both and keep the invariant. **OWNER** (8.1.1).
- *Sealing format.* AEAD in Rust with real associated data won over sealing the whole payload in Swift: plaintext never crosses to Swift to be sealed, and the format is tested on Linux.
- *Key access class.* The seal key is `WhenPasscodeSetThisDeviceOnly` with `.biometryCurrentSet`, so removing the passcode or enrolling a face destroys it.
- *Boot id.* `kern.bootsessionuuid` may be unreadable in the iOS sandbox. `BootSession` (`app/Shared/Platform/SystemClock.swift`) falls back to the boot time, then to a random id per call; each fallback ends copies sooner, never later. **UNPROVEN** which one a real iPhone uses; the enclave check screen shows it.

### 3.9 Search and service matching

Both are pure functions over the entry list a `PassStore` read when it was opened (`core/src/store/search.rs`, `core/src/store/service.rs`); each file's header gives the exact rules. There is no index file: the app and the extension each walk the store's folder names once per `PassStore`.

- **Search** looks at names only, ignoring case and accents; every word of the query must match. The order is by how well the words fit, then shorter path, then alphabetical. There is no recency, because nothing records it.
- **Site matching** is exact on host or on registrable domain by the compiled-in Public Suffix List, compared in the `xn--` form. Nothing is matched on a piece of a name. Name matches and their pairings are in 2.10.

### 3.10 Git library

The stack is `git2` (vendored libgit2, built with no TLS and no SSH of its own) with paw's own transports: russh for SSH and a small HTTPS client for GitHub (`core/src/git_transport.rs`).

**Why not pure Rust.** Signing happens on the card, so the SSH layer needs a custom signer, which rules out libssh2. russh needs ring or aws-lc for its ciphers; both contain C and assembly and do not cross-compile to iOS from Linux. Once the iOS build of the core had to run on a Mac anyway, a pure-Rust git library (gix) had no remaining advantage worth writing a push, a transport and a checkout for. `cargo test` builds the same vendored libgit2, so Linux and iOS run the same C.

What the spike showed, and the code now depends on:
- russh's external signer works for ed25519 with the card, and it sends the public-key query first. A key the host does not know is refused before the signer runs: no touch.
- One login per connection, and each git service is a channel on it. Fetch then push on one connection is one signature.
- The data to sign for user `git` with an ed25519 key is 146 bytes, so it fits a short APDU.
- The signer is synchronous. The connection runs russh on its own worker thread and the calling thread blocks in `sign`.
- libgit2 picks transports by URL scheme from a process-wide table, so the connection is handed to it for the duration of one call.
- The host key algorithms offered must be narrowed to the pinned key's type, or a host with several keys may present another one and fail the pin.
- libgit2's checkout follows a folder symlink in the working copy and treats paths as patterns unless told otherwise. paw commits from the tree, refuses paths through links and checks out literally (5.3, hostile trees).

There is no abstraction over the git library; the modules that need it call `git2` directly, most of them in `core/src/sync/`.

### 3.11 AutoFill extension

The extension is the same Swift and the same core library as the app, opened read-only on the store (`StoreForAutoFill` in `app/Shared/AutoFill/AutoFillSource.swift`). `AutoFillProvider` makes one `AutoFillModel` per request; the app's debug harness runs the same code.

- **What it reads.** One walk of the store's folder names (`PassStore`), the enrolled key's identity, and, only when a list is shown, the copies' small metadata files. Nothing is decrypted to build a list or a suggestion.
- **How it fills.** From a Face ID copy, or with the key in its own key sheet. The extension has its own `KeyPort`, so it never shares the app's held PIN and asks on every key use. A key open in the extension keeps a Face ID copy like any other. The hand-off to the app and its single-use copy (2.10) are not built: the key works inside the extension (8.2.1).
- **What it writes.** A Face ID copy, `site-pairings.json` and `one-time-codes.json`, each written whole by rename so two processes are safe. Nothing inside the repository.
- **Staleness.** A copy is served only if its sealed content id matches the `.gpg` file as it is now (3.8), so an old password is never served for a changed file.
- **No network.** The extension makes no `Setup`, `StoreSync` or `GitHubAccount`, so it never fetches, refreshes or sends. The rule is kept by what its code calls, not by the linker: UniFFI's generated Swift references every exported function when it loads, so with one library the sync code is linked into the extension. No test enforces the rule. A second library for the extension was rejected (appendix).
- **What iOS is told** (`CredentialIdentities`, the app only): path-derived services and user names, and, when the owner turns it on, which entries have a one-time code (5.6, rule 6).

### 3.12 Swift side

- **Key tasks.** A key sheet is a `KeyTaskDriver` (`app/Shared/KeyTaskDriver.swift`) following the core's `KeyTask`; whether a key sheet is up, and what it shows, is said there alone. `KeyTaskSheet` draws it and `.keySheetPresentation` presents it. A workflow starts it with a `KeyJob<Step>`: how to begin the task, its words (`KeyErrorWords.Context`), its Working line, what becomes of its end whether the sheet still follows it or let it go (Cancel, Lock now, Try again), and a hold (`BackgroundTime`) kept until the task really ends. The end is a `KeyTaskEnd<Step>`, `Step` being the workflow's own failure (`SyncError`, `SetupError`, or `Never`). Opening an entry is one job, `KeyOpen`, in the app and in AutoFill. A store's drivers all read the key through its one `KeyReader` (`KeyTaskDrivers`).
- **Stores.** `StoreRootModel` (`app/Paw/Setup/StoreRoot.swift`) shows setup until a store is set up, then Home. One key port serves both, so the PIN typed during setup is still held on Home. `StoreSession` is everything one open store has, built complete from `StoreSession.Parts` (the set-up store and the simulator's fixture differ only there): `LiveStore`, `LiveCopies` (the Face ID copies as shown; every change to them goes through it and reads the list again), the key port and reader, `StoreEdits`, `SyncModel`, the GitHub sign-in and AutoFill's suggestions. `AppModel` is the screens over one session: navigation, the key sheet that opens entries, the open entries.
- **Words.** Sentences live in words files by domain (`app/Shared/Words`, `app/Paw/Words`). The core decides what went wrong; Swift words it. What differs between the app and AutoFill is a `WordTable`, which a workflow picks when it is made.
- **Settings.** `app/Paw/Settings` holds the Settings sheet's sections, including Key diagnostics (`KeyDiagnosticsView`: what the key said, the key log, Copy diagnostics) and the touch timeout check (`TouchTimeoutCheck`), which runs as a key task and shows the key sheet's stages inline.
- iPhone: a navigation stack. iPad in regular width: three columns (`SplitRootView`). `RootView` picks by size class.

### 3.13 Threading, processes and cancellation

| Thread | Runs | Blocks on |
|---|---|---|
| `paw-key-task`, one per key task | the task: waits for the key and the PIN, runs card jobs, then the work after the key (network, git) | the card, up to its touch timeout; the network |
| `paw-touch-grace` | the 400 ms wait before `Touch` shows for a cached policy | a timer |
| `paw-card-forget` | de-verifying the card after paw forgot the PIN | the card |
| a tokio worker per SSH connection | russh | the network; never the card |
| the caller's | reads, search, saving, Face ID opens | file work; Face ID in `unseal`, so Swift calls it off the main thread |

- No exported call waits for a PIN or a touch: every such wait is a `KeyTask` with stages and a cancel. A blocking call with a prompt callback was rejected (appendix).
- `UsbCardConnection.transmit` gives up after 40 seconds, well over the key's own touch timeout, as the last defence.
- A listener is told every stage change in order, on whichever thread made it, and must not block; `MainQueueTaskListener` only queues to the main queue.

**Two processes on one container.** The app and the extension share the state directory and the copies folder.

- **The store lock** (`core/src/lock.rs`). Per store, a readers-writer gate in the process plus an advisory `flock` on `store.lock` in the state directory between processes. Every wait is bounded; past it the operation fails with `StoreBusy` and changes nothing. The kernel drops a dead holder's lock.
- **Why it is held only for local file work.** iOS kills a process that is suspended while holding a file lock in a shared container (`0xdead10cc`). So the store lock is never held across a transfer, a wait for the key or a person, or a callback into Swift. A fetch writes objects and refs into a private namespace under an in-process fetch lock, and takes the store lock only to publish them, integrate and check out. A push sends with no store lock and takes it to record the result. Logins happen before any lock. `core/src/lock.rs` lists the holders and the lock order.
- **Evidence, not re-entry.** Code that changes a store takes an `Exclusive` as a parameter and code that reads under the lock a `Shared`, so neither can be called without the lock.
- **Being sent.** A send records in memory the commit going out, so discarding a change that is on the wire is refused instead of appearing to work.
- **Only the app fetches and sends.** The fetch and push locks do not reach across processes, which is one reason the extension must never do either.
- **Everything else is written whole by rename,** with no lock: state files, Face ID copies, pairings and one-time-code marks. A reader sees a whole old file, a whole new one, or none. Two processes writing one list in the same instant can lose one item, which costs one more question or one missing suggestion, never a wrong fill. A copy reset by one process while the other writes can only lose that copy.
- **Token refresh** is serialised in the process and takes no lock between processes; the race is handled after the fact (`core/src/github/session.rs`).
- **Risk left:** a process suspended inside a store-lock critical section (most likely the first clone's checkout of a large store) holds the `flock`. The app runs key tasks and sync inside background time (`BackgroundTime`) for that reason; it has not been observed on the device.

### 3.14 Errors

Each workflow has its own error type, exported to Swift: `KeyError` (the key; `core/src/key/mod.rs`), `SyncError`, `SetupError`, `WriteError`, `GitHubError`, and `PawError` for reading the store and Face ID copies. What can go wrong underneath any of them is one shared case, `Fault` (`core/src/fault.rs`): the store is busy, the remote failed (`RemoteFailure` in `core/src/remote.rs`, said once for setup, sync and the GitHub sign-in), a refused path, or a local failure that is only text. A fault keeps its meaning however many layers it passes through.

**Where wording lives.** In Swift. The core decides what went wrong and carries the facts as fields; its own texts are for logs and tests. The exceptions are the form's problems (`DraftProblem`, `OtpProblem`), which are written beside the rules they explain and shown as they are. Error values never carry a PIN, plaintext or APDU data.

**Status words.** The session's transport wrapper records the instruction and status word of every exchange, so a failure the card library reports as text is classified from the status word (`core/src/key/apdu.rs`).

**Touch timeout versus lost verification.** Both produce `6982` (or `6985`) on DECIPHER and INTERNAL AUTHENTICATE. After such a refusal, in the same card job, the session sends an empty VERIFY, which uses no try:

- Not verified: the held PIN is dropped and the PIN stage comes back ("Your key asked for its PIN again"; no try was used). The held PIN is not re-sent: something reset the card mid-job. A second loss in the same task fails it, so it never loops. A signature, which cannot ask while the server waits, fails with `PinForgotten`.
- Still verified, on a slot that can ask for a touch, after 1 second or more: `TouchTimeout`, PIN kept.
- Still verified but quicker than 1 second: not a timeout, since a YubiKey waits about 15 seconds for a touch and "touch the gold part while it blinks" would be wrong advice. It fails as the key's own refusal with the status word, PIN kept. So does a refusal on a slot that never asks for a touch.
- A card that cannot answer the empty VERIFY is judged on the time alone.

**UNPROVEN** on real firmware: what a YubiKey answers after its touch timeout, and to the empty VERIFY then. The touch timeout check in Settings › Key diagnostics exists to find out, and the key log records both.

**Key log** (`core/src/key/keylog.rs`). Each process appends the lines of its key sessions to its own small file in the state directory, so the app can show what happened in the extension too. A line holds a card command's instruction, P1, P2, status word and timing, or a task's purpose, stages and the name of its error, or a session event. Never command or response data, the PIN or its length, entry or host names, or an error's text. From the PIN stage until the VERIFY that carries the PIN, lines get whole-second times, so the log does not time the typing.

### 3.15 Data on disk

Everything is under one folder, `paw/`, in the App Group container `group.com.marktoda.paw` (`StoreLocation`). The unsigned simulator build has no App Group and uses a folder in the app's Application Support instead.

```
paw/
  store/          the core's state directory; its files are named in core/src/files/layout.rs
    repo/         the store: working copy and .git (ciphertext only)
  face-id/        Face ID copies, one small file each
```

- The state directory holds the setup step, the confirmed host keys, the store's address, the chosen GitHub repository, the enrolled key, the pinned keys and accepted `.gpg-id` files, AutoFill's pairings and one-time-code marks, the key log and `store.lock`. None of it is secret; none of it is in the repository or synced.
- Copies are kept apart from the store so the store can be set up again without touching them, and the other way round.
- **Protection.** Every folder and file under `paw/` is given `NSFileProtectionComplete`, so the store is unreadable while the iPhone is locked, and the folder is excluded from device backups. That is why a background refresh skips while locked. Never observed on the device (roadmap, "Known gaps").
- **Keychain.**
  - The copies' seal key: Secure Enclave P-256, `WhenPasscodeSetThisDeviceOnly` with `.privateKeyUsage` and `.biometryCurrentSet`, in the App Group's keychain group so a copy made in one process opens in the other.
  - The copies' index key: a random secret readable without a prompt while the phone is unlocked. Same group.
  - A record per process of the enrolled faces.
  - The GitHub tokens: one generic-password item, `AfterFirstUnlockThisDeviceOnly`, not synchronised, in the app's own group, which the extension cannot read.
  - Nothing else. No PIN and no plaintext.
- `LegacyStore` deletes what an earlier build kept in the app's own Application Support and keychain group. Nothing is moved over.

### 3.16 Testing

| Layer | Where | What |
|---|---|---|
| Core | `core/tests/`, plus unit tests beside the code; Linux, `cargo test` | Everything in the core against the simulated key, keystore and clock, GnuPG and git. Each file's header says what it covers. Sync runs against two clones of one bare repository behind the in-process SSH host; GitHub against the simulated GitHub; hostile trees on a real case-folding file system in a user namespace; what paw writes is read back by `gpg` and git |
| Core without `sim` | the same job | `cargo test --no-default-features`: the core as devices get it |
| State files | `core/src/files/fixtures.rs` | Every kind of state file as an installed store has it: each still reads, and what paw writes is byte for byte what it wrote before |
| Swift models | `app/PawTests`, hosted in the Debug simulator app | The models and the wording, on the core's simulated key and fixture store in-process, with no UI (`Fixture`) |
| Screens | `app/PawUITests`, XCUITest in the simulator | Flows on the fixture store (`-pawFixture`), setup against the simulated git host (`-pawSetup`), AutoFill through the provider's root view hosted in the app (`-pawHostProvider`). `PawUITestCase` has the shared helpers; waits are on conditions, never on time |
| Device builds | CI `device` job | `scripts/check-no-sim.sh` on the library, the app and the extension (5.5) |
| Device | by hand | The checklist in the roadmap ("Needs the device") |

The real Secure Enclave path, the real extension process, the system Face ID prompt and the suggestion bar cannot be tested in the simulator. The enclave check screen (`app/Paw/Debug`) and Key diagnostics exist to make the device checks short.

Every state reachable on a device should be reachable in the simulator through a launch argument or a simulator control. A flow that cannot be scripted there is not finished.

---

## 4. Unlock and git auth as pluggable providers

### 4.1 Unlock

**There is one unlock provider, the OpenPGP card, and no provider trait.** A trait shaped as "ciphertext in, plaintext out, prompting through a callback" was considered and dropped: it puts a second owner of the PIN and touch sequence beside the key task, its blocking call would hold the card while a person types, and its only implementation would be the card. The next likely provider (NFC) does not fit the shape either: PIN first, then tap, re-verify every time. The trait is extracted when a second provider exists, from two real cases.

What keeps that extraction cheap:

| Seam | What it is |
|---|---|
| Applet-agnostic transport | `CardConnection` exchanges raw APDUs and knows nothing of OpenPGP |
| The sheet renders stages | `KeyTaskState` says plug in, PIN, touch, working; the sheet decides nothing |
| The key knows no workflow | `core/src/key/` does card jobs; `core/src/task/` knows what they are for |
| Copies are not a provider | The copy cache holds plaintext copies, not a key, and sits in front of whatever unlocks (`core/src/open.rs`) |
| The enrolled key is a record | Identity by fingerprints, in a file of its own (`core/src/key/enrolment.rs`) |

A software decryption key exists nowhere outside the `sim` feature: multi-recipient test entries are encrypted to the simulated card plus throwaway public keys.

| Later provider | New work | Open question |
|---|---|---|
| YubiKey over NFC | A Swift NFC reader implementing `CardConnection`; a stage order of PIN, then "Hold your key to the top of your iPhone"; every decrypt re-verifies from the held PIN because the field drops between taps. Not possible in the extension. Also the route to Lightning iPhones | **UNPROVEN:** that NFC presence satisfies the touch policy |
| Software key | A secret key wrapped by the device keystore; import or generate screen. Rule 1 of 5.6 must be rewritten as an explicit opt-in; this is when the provider trait is extracted | |
| Secure Enclave key as recipient | A P-256 ECDH OpenPGP subkey whose private operation is a foreign call. Adding the recipient means re-encrypting, one touch per entry | **UNPROVEN:** that rPGP accepts an external P-256 ECDH provider |
| age / passage | A second cipher format and store format; age on a YubiKey uses the PIV applet, which the transport already allows | |

### 4.2 Git auth

- **`SshSigner`** (`core/src/key/signer.rs`) is what signs an SSH login: a public key and a synchronous `sign` that may block for as long as the key waits for a touch. The one product implementation is `SessionSigner`, which signs with the card's authentication key through the key session. Only `ssh-ed25519` is supported; the card signs the data as given, which is what that algorithm wants.
- **`Remote`** (`core/src/sync/transport.rs`) is where the store lives: host, port, user, path and the pinned host key. `Connection::open` connects, checks the host key against the pin and logs in with one signature; `Connection::probe` asks the host whether it would accept a key, with no signature. One `Connection` serves several git channels, which is what makes fetch, replay and push one touch.
- **The signer is called only for the login of a connection paw opened itself, after the host key has been verified.** No generic "sign this" call is exported, and there is no agent forwarding.
- **GitHub over HTTPS** is not a second signer. It is a separate, read-only transport with the sign-in's token (`core/src/github/transport.rs`), which refuses `git-receive-pack` before anything is sent. Both transports reach libgit2 the same way and share one download cap (`core/src/git_transport.rs`).
- **A device key** (a Secure Enclave P-256 key as a read-only deploy key, for hosts other than GitHub) would be a second `SshSigner` with no prompt. It is not built. **UNPROVEN:** ECDSA P-256 through russh's external signer, and a way to confirm on hosts other than GitHub that such a key is read-only.

### 4.3 How each consumer uses the contracts

| Consumer | Unlock | Git auth | Prompts |
|---|---|---|---|
| Open an entry (app, extension) | Face ID copy, else a key task (`KeyPort.open_entry`) | none | Face ID or the key sheet |
| Setup: host check | card read only | `Connection::probe` with the card's public key | none; first-use host confirmation |
| Setup: download | none | key task, card signature | key sheet |
| Setup: check | key task; pins and binds recipients on success | none | key sheet at Touch |
| Save, delete, rename, move | none (pinned public keys) | none | none |
| Send | none | key task, card signature; fetch and push on one connection | key sheet |
| Check for changes, signed in to GitHub | none | read-only token over HTTPS | none |
| Check for changes, otherwise | none | key task, card signature | key sheet |
| Background refresh | none | read-only token over HTTPS | never prompts; decisions wait for the person |
| Re-encrypt held-back changes | key task, one touch per change | none | key sheet |
| The gold disc on a row | `Access` and the copy list; no card, no prompt | none | none |

---

## 5. Security model

### 5.1 Goal

The decryption key exists only on the YubiKey. Reading an entry takes the key, its PIN and one touch, and that touch releases exactly one entry. The single deliberate exception is the Face ID copy. Everything else on the phone (the clone, the names, the GitHub token) is worth no more than a read-only copy of the git repository. paw makes two kinds of network connection: SSH to the configured remote, and, only once the person has signed in to GitHub, HTTPS to `github.com` and `api.github.com`.

### 5.2 Guarantees and non-guarantees

| Adversary | paw gives | paw does not give |
|---|---|---|
| Unlocked phone, no key, not the owner's face | No entry plaintext. Enrolling a new face destroys the seal key. Cannot push. Local changes are named in the queue before the owner's key sends them | Names, folders and the covered list are visible. New entries, deletes and renames can be queued. Pasteboard within its expiry. An entry left open less than 2 minutes ago is still open |
| Unlocked phone with the YubiKey | No persisted PIN. If the PIN is not held: three guesses, then the card blocks | If paw's process still holds the PIN (same plug-in, under 15 minutes in the background), each touch opens one entry with no PIN. A device lock does not clear the PIN. Phone + key + PIN is full compromise by design |
| Locked phone | The store's files, the copies and their key are unreadable while locked. No notification carries an entry name | The GitHub token is readable after first unlock. A held PIN or a recently open entry may remain in a suspended process's memory until paw next runs |
| Malicious sandboxed app | Cannot read the container, keychain group or memory. Third-party keyboards are refused everywhere in paw. paw holds the reader's exclusive session while the key is in | Pasteboard on paste. Another smart-card app can block the PIN or imitate the sheet. A jailbroken device is out of scope |
| Someone with the repo | Contents are confidential to the recipients | Every name, folder, file size, edit time, commit message and the full history. No forward secrecy, no post-quantum protection |
| Someone who can write to the repo | Recipient pinning, including for queued changes; history-rewrite detection; hostile-tree checks; a download cap; `url` cross-check before filling | Ciphertext swap between names is caught only for entries with a `url` field. Deletion, rollback to a descendant-consistent state and withheld updates are undetectable |
| Network attacker | SSH with pinned host keys; no credential offered to an unverified server; HTTPS to GitHub with certificates verified and the token sent to GitHub's origin only | Sees that the device talks to the git host and when. Can block sync |
| Shoulder surfer, recording | Masked by default; a revealed value hides again after 30 seconds; blank app-switcher cover | Names on screen. Screen recording and screenshots are not detected |
| Hostile USB reader | The PIN is sent only to a card presenting the enrolled fingerprints | Those values are public and spoofable; the PIN goes in clear to whatever answers. The PIN alone opens nothing |
| Compromised build or dependency | Nothing intrinsic; mitigations are procedural (5.7) | A malicious build can exfiltrate every entry the user opens. The private key stays safe |
| Coercion | Lock now, with no confirmation and no undo; holding the side buttons disables biometrics | No guarantee |

A phone thief can queue deletes, renames and new entries, because those need no key. The thief cannot push, every local change is named in the queue before the owner sends it, and Add refuses to overwrite an existing name. Editing an existing entry needs its plaintext.

After losing a phone, the user should revoke paw's GitHub authorisation and rotate the entries that had live copies that day. There is no remote wipe beyond Find My.

### 5.3 Mechanisms not already covered

**Face ID copies.** As in 3.8. On a device there is no software fallback: with no passcode, no enrolled biometrics, or a key that cannot be made, copies are unavailable and every open uses the key.

**Recipient pinning.** As in 3.7. A `.gpg-id` added, changed or removed by a pull is never used for encryption until the person accepts it in the foreground; until then paw refuses to write in that folder. Refusing won over "keep encrypting to the old list" because it is simpler and cannot silently diverge from other clients. A queued change made before such a pull is held back, not pushed (3.6).

**SSH host keys** (`core/src/sync/hosts.rs`).
- Bundled, with no prompt: github.com (and its port-443 endpoint), gitlab.com and bitbucket.org, each from the host's own documentation, with a test that every key has the fingerprint written beside it. codeberg.org and git.sr.ht publish fingerprints but not keys, so they are confirmed on first use like any other host.
- Other hosts: trust on first use, after the person has seen the fingerprint and confirmed it, stored per host and port.
- A mismatch is a hard failure with no bypass inside the connect flow. The error shows the expected and presented fingerprints.
- The only way past it is to forget the pinned key on purpose (Settings, or setup's own step). A bundled host's keys cannot be forgotten: a change there waits for an app update.
- Background work never prompts.
- russh is built without RSA and without compression. The algorithms offered are the pinned key's.
- When github.com cannot be connected to at all on port 22, paw offers GitHub's port-443 endpoint and uses it only when asked.

**Store address** (`core/src/sync/address.rs`). GitHub's short and `https://` forms become the SSH address, with a notice. Refused: a password, token or user name in the address (never repeated back), git's other transports, and anything that could reach a program as an option.

**Hostile trees** (`core/src/sync/guard.rs`). The repository comes from a host that may be hostile, and APFS treats names that differ in case or Unicode normalisation as one name; a tree with a link and, later, a file "under" it spelled differently could make a checkout write outside the store, over paw's own state. In layers:
- Every tree is checked before it is checked out or committed: the remote's tip at clone and integrate, every commit a replay rebuilds, every tree a discard checks out. Refused: links, submodules and any mode but folder and regular file; unsafe names (`..`, `.git` in any disguise, names that read as `.gpg-id` but are spelled otherwise); any two paths that are one name on an iPhone; and trees past fixed bounds on depth, path length and size. `guard.rs` has the exact list and what it does not cover.
- Every repository paw opens or creates gets settings that turn off links and line-ending conversion and an attributes file that outranks any `.gitattributes` in the tree. Without it a remote `.gitattributes` did rewrite ciphertext on checkout.
- A refusal leaves branch, working copy and queue untouched and names the paths.
- Reading never follows a link either (`core/src/store/read.rs`).

**Download cap.** Each fetch or clone may receive at most 256 MiB, counted as bytes arrive, before libgit2 writes the pack. Past it the transfer stops and nothing is integrated.

**GitHub sign-in** (`core/src/github/`; the module's header lists exactly what is sent where). paw's GitHub App has no client secret and is signed in to with the device flow. The token is read-only on repository contents and is never used to push. The access and refresh tokens never reach Swift: Swift only provides where they are kept (`TokenVault`). They are one keychain item replaced whole on each refresh, so rotation is one write. Credentials are attached only to a URL whose scheme, host and port are exactly GitHub's, and redirects to another origin are not followed. A refresh refuses to run unless the store's remote is the repository chosen in the picker. Sign-out can revoke both tokens; removing the app's authorisation is done on github.com, because it needs a client secret. Not verified against the real GitHub: token renewal after 8 hours (roadmap).

**Clipboard.** Every secret is written local-only with an expiry (`app/Paw/Clipboard.swift`). On return to the foreground paw also clears it if the pasteboard is still paw's and the time has passed. No "copy whole entry", no copy on open. The extension never touches the pasteboard.

**Screen privacy.** Masked fields and the app-switcher cover of 2.7. Hiding a revealed value while the screen is recorded, and the screenshot warning, are not built. Edit fields disable autocorrection.

**Logging.** No log line, error string or crash annotation contains plaintext, the PIN, APDU data, session keys, ciphertext or pasteboard content. Secret-bearing Rust types have no `Display`, a redacting `Debug`, and wipe their memory on drop. The key log (3.14) and Copy diagnostics (`core/src/report.rs`) say what they may hold and a test checks the diagnostics name nothing. No third-party SDKs, no telemetry.

Known limit: the HMAC state that makes a one-time code (`hmac` 0.12, which has no zeroize support) is not wiped on drop; the secret and the code are.

**System stores.** No CoreSpotlight, `NSUserActivity`, Siri suggestions or widgets with names. `ASCredentialIdentityStore` holds path-derived data, the domains the person confirmed a name on, and, when turned on, which entries have a one-time code. It is cleared on store removal.

### 5.4 How copies weaken "hardware key required"

For a covered entry, until its deadline:
1. The sufficient factor becomes "this phone plus the enrolled face". Its security rests on Apple's Secure Enclave and Face ID, not the YubiKey.
2. Plaintext-equivalent data is at rest on the phone.
3. Expiry is software: an ended copy's file is wiped the next time paw touches the cache, and refused by a check before that. The seal key itself is destroyed only when all copies end at once (Lock now, a reboot, a clock set back).
4. Biometrics can be compelled in ways a PIN cannot.
5. The extension process can open covered entries.
6. There is no remote revocation.

One addition beyond copies made by a key touch: a new entry is copied without any key touch, for the configured length from the save.

Copies do not weaken entries that have none, the private key, the PIN, or git write access. The device passcode alone never opens a copy. Never describe copies as "locked by your key" or "hardware protected".

### 5.5 Keeping the simulated key out of device builds

The simulated key's private scalar is a constant in a public repo. A device build that could enrol it would encrypt entries to the world. `SimKeystore`, the software stand-in for the Secure Enclave, is as dangerous. The layers:

1. **A cargo feature.** All simulated hardware (`SimulatedKey`, `SimKeystore`, `SimClock`, `SimRemote`, `SimGitHub`, the fixture store) is in `core/src/sim`, behind the feature `sim`. It is on by default, so tests and the simulator have it. `scripts/build-core.sh` always builds the device library with `--no-default-features`.
2. **Bindings per slice.** UniFFI's Swift checks every exported function's checksum when it loads, so bindings made from one library cannot be linked against the other. Each library gets its own: the device bindings inside `#if !targetEnvironment(simulator)`, the simulator's inside `#if targetEnvironment(simulator)`, matching the xcframework's slices. A device build sees only the device bindings whatever its configuration.
3. **Swift.** Everything that names a simulated type, and every harness, is under `#if DEBUG && targetEnvironment(simulator)`; the launch arguments, the gallery and the enclave check are under `#if DEBUG`. The device build is Release.
4. **CI proof.** `scripts/check-no-sim.sh` fails the `device` job if the device library, the Release app or the extension contains the simulation's symbols, the fixture's passwords or the simulated host's key, any declared launch argument, or the name of a type the simulator, debug and gallery folders declare. With `--expect` it runs on the simulator build, where every pattern must match, which keeps the patterns honest. Settings › About shows the commit and configuration.

A separate crate for the simulation was the first plan; the feature gives the same result with one library to build and the scan to prove it.

Not built: a runtime refusal, compiled into device builds, to enrol or encrypt to the simulated key's fingerprint. The App Store "Try with demo data" path (2.4) conflicts with all of this and needs a separate design.

### 5.6 The never-break rules

1. **The private key and anything that substitutes for it stay on the card.** No exported key, no cached session keys, no software decryption key in any device build. The only plaintext-equivalent at rest is a sealed copy.
2. **One human gesture releases one named entry.** No batch, background, speculative or list decrypt exists. (A key whose touch policy is "cached" or "off" weakens this at the card; paw says so at setup.)
3. **Plaintext reaches disk only as a sealed copy** under a Secure Enclave key with `biometryCurrentSet` and `WhenPasscodeSetThisDeviceOnly`. No passcode fallback. Edits are encrypted before they are written.
4. **Check the deadline before every copy read and fail closed.** Face ID use never extends a copy. A clock that went backwards or a reboot ends all copies. Ending all resets the keystore first.
5. **The PIN is never persisted, logged or shared with the extension, and never retried automatically.** Send it only to a card presenting the enrolled key's fingerprints, and never as typed to a card whose KDF expects a hash. When paw forgets the PIN, de-verify the card. No PIN try is spent on a key that cannot do the thing requested.
6. **Nothing learned from decrypted content is written anywhere except a sealed copy.** The identity store, site pairings, logs and git metadata hold path-derived names and domains the person confirmed only. One exception, off until the owner turns on "Suggest one-time codes" (2.10): a list of entry paths that have a one-time code, and a one-time-code identity for each of their sites. It says which entries have a second factor, never the factor; turning it off deletes both.
7. **Every pasteboard write of a secret is local-only with an expiry.** The extension never uses the pasteboard.
8. **Encrypt only to recipients pinned on this device, and never to fewer than `.gpg-id` names. This covers ciphertext already queued:** a change encrypted before its `.gpg-id` changed is not pushed. A remote `.gpg-id` change is untrusted until confirmed. Never encrypt to the simulated key on a device.
9. **Never send a credential to an unverified host, and never bypass a host key mismatch inside the connect flow.** Card-authenticated operations are foreground and user-initiated. Background work uses only the read-only GitHub token and only fetches. The extension never uses the network. paw contacts no host other than the git remote and, once signed in, GitHub.
10. **Simulated hardware cannot exist in a device build, and CI proves it on every build.**

Engineering invariants alongside these:
- One card job at a time per key session.
- Only the app fetches and sends. The extension writes Face ID copies, pairings and one-time-code marks, and nothing in the repository.
- The store lock is never held across the network, a wait for the key or a person, or a callback into Swift.
- Saving never needs the key or the network.

Rule 6 excludes remembering the `login` and `url` of opened entries for the AutoFill bar. The bar and row subtitles use path-derived data, plus the domains the person confirmed.

### 5.7 Build pipeline

`.github/workflows/build.yml`:
- `core` (Linux): `cargo test` with and without `sim`, both sets of Swift bindings, and the licence notices check.
- `core-ios` (macOS): `scripts/build-core.sh`, the two iOS libraries.
- `plan` and `ui` (macOS): the UI tests split across machines, with the iPad tests as their own part on an iPad simulator, and the unit tests in one part (`scripts/ci-test.sh`).
- `device` (macOS): the Release build for devices, unsigned, scanned (5.5) and uploaded.

The app is signed and installed from the owner's Linux machine (`scripts/install.sh`), so the signing key never leaves it. CI artifacts of a public repo are public: tests use only the simulated key and obviously fake fixtures.

Not done yet, and wanted before anyone else depends on a build:
- Actions pinned to commit SHAs (they are pinned to tags), and XcodeGen pinned to a version.
- `permissions: contents: read` for every job; never `pull_request_target`; no signing material in a workflow that runs on pull requests.
- `cargo build --locked` and `cargo test --locked`, and `cargo deny` so a new crate with a C build dependency is an explicit decision.
- Device builds only from a commit on `main`, with no restored caches and a provenance attestation that `scripts/install.sh` verifies before signing.
- Review `ITSAppUsesNonExemptEncryption: false` in `app/project.yml` before any App Store submission. libgit2's bundled xdiff is LGPL: decide before any listing (roadmap).

---

## 6. Extension points

What is deliberately left for later, and where it would attach. What is built is in section 3; what is planned is in the roadmap.

| Item | Decision | Note |
|---|---|---|
| YubiKey over NFC | Leave a seam | 4.1. Also the route to Lightning iPhones |
| Software key; Secure Enclave key as recipient | Leave a seam | Extract the provider trait then; rule 1 must be revised deliberately |
| Several recipients: import a public key, recipient editing, sharing | Leave a seam | The keyring file already has room for keys from a certificate, which nothing reads yet. Realistically sharing is a second store, since a subfolder exposes every name |
| Replace my key | Leave a seam | Designed in 2.12, not built. The enrolled key is one record |
| age / passage | Leave a seam | `pgp` is used in `core/src/key/` and `core/src/write/` (and by setup and the fixture); the store format's names are in `core/src/path.rs` |
| Deploy key for hosts other than GitHub | Leave a seam | A second `SshSigner` (4.2) |
| Hand-off from the extension to the app; single-use copies | Leave a seam | Only needed if an iOS update stops the extension reaching the key (8.2.1) |
| Field-level compare of two kept versions | Leave a seam | Both versions exist as entries; a compare view needs both open |
| One-time codes by QR code | Leave a seam | Codes are built (`core/src/otp.rs`); the editor takes a base32 key or an address |
| Passkeys | Leave a seam | A further request kind in `AutoFillRequest` |
| Password generator options, word mode | Leave a seam | The generator already takes a policy; a word mode needs a word list with a clear licence |
| Unblock the PIN with the admin PIN on the phone | Leave a seam | See the appendix |
| Folder rename and delete | Leave a seam | One change per entry; needs confirmation design |
| Batch operations (re-encrypt a store for a new key) | Leave a seam | One touch per entry bounds how good this can be |
| Commit signing | Leave a seam | One commit function (`core/src/write/commit.rs`); a further `KeyPurpose` |
| Onboarding for strangers | Leave a seam | Further setup steps (create a store, generate a key); demo data (5.5) |
| Saving a password from the extension | Leave a seam | Needs iOS 26.2; the minimum is 26.0 |
| Mac app | Leave a seam | Expect contention with `gpg-agent`/`scdaemon` |
| Stored PIN behind Face ID | Ignore for now | It turns "key + PIN + touch" into "key + face + touch"; needs its own opt-in |
| Multiple-stores UI | Ignore for now | Every core call takes a state directory, so a second store is a second folder |
| Health checks, breach lookups | Ignore for now | Touch-per-decrypt limits audits to opened entries; lookups conflict with the privacy stance |
| `.gpg-id` signatures, signed-commit verification | Ignore for now | The real fix for a hostile writer |
| RSA cards | Ignore for now | Needed before a public release; the simulated key covers Curve25519 only |
| `paw://` URL scheme, Shortcuts | Ignore for now | Must not trigger a decrypt |
| A wizard engine, a plugin registry, abstractions over the git library, the cipher format, the identity or the store format | Ignore for now | One implementation each |
| Async UniFFI | Ignore for now | Key tasks with stages cover it |

---

## 7. Build order

The milestones and their status are in [roadmap.md](roadmap.md). This is why they came in that order.

- **The git and SSH spike came first** (3.10). Whether the card can sign an SSH login through a Rust SSH library that builds for iOS was the largest schedule risk, and all of sync rests on it.
- **Foundations and the key session (M1) before any screen that uses the key.** Answers from the real key decide how the session holds and clears verification and how cancel works. The App Group container and the shared keychain group were settled here too, because the extension dictates both and everything after writes there.
- **Read a store (M2), then Face ID copies (M3), then clone and setup (M4).** Each is usable on the fixture store before the next exists. After M4 the app is usable read-only every day and stays current.
- **Fetch, fast-forward and replay land with clone and push (M4, M5), not with silent pull (M6).** Otherwise the first desktop commit would make every phone push a non-fast-forward with no recovery.
- **Editing and push (M5) before full AutoFill (M7)** (principles.md). M7 depends only on M3 and M4, so it could have moved ahead.
- **iPad (M9) follows iPhone:** the same models and tests on a split view.
- As much as possible lands in the core, because Rust is tested on Linux in minutes and Swift compiles only on CI.

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
| 4 | `endSession` interrupting a pending touch wait | How cancel feels | Close the sheet at once; `Busy` until the card's 15 s timeout runs out |
| 5 | INTERNAL AUTHENTICATE payload size over `TKSmartCard.transmit` | SSH auth with the card at all | The simulator supports chaining and extended length; use whichever the device accepts |
| 6 | Two session channels on one authenticated connection at the git host | One touch or two when the server moved | A second connection and touch in that case only |
| 7 | App Group container and shared keychain group in the simulator with `CODE_SIGNING_ALLOWED=NO` | Every XCUITest from M2 on | Ad-hoc sign the simulator build with entitlements |
| 8 | Face ID for a Secure Enclave key inside the credential provider | Copy fills in AutoFill | None; without it AutoFill is hand-off to copy-paste. Found in M3b |
| 9 | A credential provider opening its containing app; a cheap way to run the reach probe in the real extension | Not relied on | Instruction text, as designed |
| 10 | Extension memory with one library | The 120 MB limit | Trim the index; split a library only if measured necessary |
| 11 | Pasteboard expiry after termination; the AutoFill enable prompt API | Copy toast accuracy; setup S6 | Clear on next foreground; **Open Settings** |
| 12 | Read-only probe of the deploy key on hosts other than GitHub | Background-updates self-check | Show "paw couldn't confirm this key is read-only" and continue |
| 13 | Deleting a `WhenPasscodeSet` keychain item while locked; `BGAppRefreshTask` timing | None: background code never touches the copies folder | Correctness rests on the read-time check |
| 14 | Reading "cached" touch policies; the library's `touch_required()` reduces them to a boolean | The S1 lines | Read the policy bytes directly |
| 15 | The `ykman openpgp access unblock-pin` and `addcardkey` wording in user-visible strings | Copy | Verify against current tools before shipping |
| 16 | Whether the `openpgp-card` library handles KDF itself | Whether v1 must refuse KDF cards | Refuse, as designed |
| 17 | NFC presence satisfying the touch policy; rPGP external P-256 ECDH | Later features only | None needed now |

---

## Appendix: Considered and rejected

| Proposal | Why it was not adopted |
|---|---|
| Zeroise the held PIN shortly after paw leaves the foreground (at background-task expiry or device lock) and change the copy to "Asked again after you leave paw" | It contradicts the owner's decision of one PIN per plug-in session: with a 6 to 127 character PIN it means typing it on nearly every use. The accurate parts were adopted: the design does not claim the PIN is gone at device lock, states that a suspended process keeps it in memory until paw next runs, applies the limit before any card job on return, and puts the stricter rule to the owner (8.1.2) |
| "Unlock with admin PIN" inside the key sheet in v1 | Three wrong admin PINs leave the key recoverable only by a factory reset that destroys the private keys. That is a worse outcome than waiting for a computer, the flow also needs a set-new-PIN step, and it would be the one card flow whose failure cannot be undone. Left as a seam. The other half of the same point (show tries left before any input) was adopted |
| Delay Lock now behind a 5-second **Undo** toast | Lock now is the panic control and must be immediate and irreversible. An accidental tap costs touches, not data. It was moved into its own destructive section of the menu, away from Sync now |
| One thread that owns the card and runs every job, with separate card and network state machines behind each sheet | A thread per key task and a per-session gate that lets one card job run at a time gives the same guarantee with one state machine per task, and a task's network work can simply run in its own thread after the card's part |
| A blocking core call that asks for the PIN through a callback | It needs a blocking callback from Rust to the main thread, cancel becomes awkward, and a rebuilt view cannot re-read where things stand. Every wait on a person is a task with stages and a cancel |
| A provider trait with a blocking `decrypt` and a prompt callback; a software identity for tests | See 4.1: a second owner of the PIN and touch sequence, a held card while a person types, one implementation. Test fixtures use throwaway public keys |
| A pure-Rust git library | See 3.10: the SSH library needs C either way, so the iOS build of the core runs on a Mac, and a push, a transport and a checkout would have had to be written by hand |
| A second static library for the extension so that no sync code is linked into it | With the device and simulator variants that is four libraries and four sets of bindings. The extension simply never calls sync (3.11) |
| A separate root object for the extension with a narrower API | The extension uses the same exported types as the app; what it may do is decided where it opens the store (`StoreForAutoFill`) |
| A separate crate for the simulated hardware | See 5.5: a cargo feature plus the device scan gives the same result with one library |
| A store lock held across a transfer, or no store lock at all | A process suspended while holding a file lock in the shared container is killed by iOS; with no lock, two refreshes at once failed on a ref's lock file. The lock exists and is held for local file work only (3.13) |
| A user-facing three-way conflict choice (keep mine, keep theirs, keep both) | Both versions are ciphertext, so the choice is a guess between two timestamps and a wrong guess discards a password. Both are kept automatically and the user deletes one after looking |
| Parking a held-back change in a ref of its own, or re-encrypting it automatically | Re-encrypting needs the key, one touch per change, so it cannot happen without the person. The change stays in the queue, where it is listed, and the send is refused while it is there |
| One shared window with a fixed end; a sliding end | See 3.8: fixed strands late-opened entries; sliding lets a copy outlive the chosen length |
| Copies keyed by the git blob id alone | The file name would say which entry a copy is of to anyone who has the repository. Names are a keyed hash of the path, and the content id is sealed inside |
| Sealing a whole copy in Swift with the Secure Enclave key | See 3.8: the plaintext would cross to Swift to be sealed, and the format could not be tested on Linux |
| An index file of names for the extension | Walking the folder names once per request is fast enough at the sizes tested, and a file can go stale |
| Auto-presenting the push sheet after a save when the key is plugged in | Five saves would be five interruptions, and it breaks "nothing presents itself" |
| Remembering `login` and `url` from opened entries to improve AutoFill suggestions | Breaks rule 6. Name matches with pairings the person confirmed give most of the benefit without storing decrypted content |
| A "Share with my other devices" clipboard switch | It turns off local-only and weakens rule 7 for little gain |
| Wording errors in the core and returning finished sentences | The app and the extension word the same failure differently, and a sentence fixed in Rust cannot follow the screen it is on. The core carries the facts; the words files word them (3.14) |
