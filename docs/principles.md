# paw principles

*First draft, 2026-11-22. Written for the owner to edit: change or delete anything that is not what you mean.*

paw is an iPhone and iPad app for people who keep their passwords in [pass](https://www.passwordstore.org/): one GPG-encrypted file per entry, in a git repository they host themselves. The key that decrypts lives on a YubiKey and nowhere else. paw is for people who would rather hold their own key than trust a vendor's servers, and who still want an app that is easy to set up and pleasant to use.

## Principles

### 1. Keep the key with the person

The private key is generated or stored on a hardware key the person holds, and decryption happens on that key. paw never sees the private key, so paw cannot leak it, back it up or recover it.

*So we won't* add a master password, a key stored in the app, or a "recover my vault" feature. Recovery means a second key or a backup the person made themselves.

### 2. Need no server, trust no server

There is no paw account, and paw works in full with nothing but the person's own git remote, or with no remote at all. The app talks to that remote and to nothing else unless the person turns something on.

*So we won't* ship analytics, third-party crash reporting, or any feature that only works through a service we run. *We may* later offer an optional helper service, for example to keep a copy available, but only if it holds nothing except what the git host already sees, can be replaced by something the person runs, and can be switched off without losing anything.

### 3. Use formats that outlive the app

A paw vault is a normal pass store: OpenPGP files, a `.gpg-id` file and a git history. Anything paw writes must be readable by `pass`, `gpg` and `git` on a laptop, with no paw involved.

*So we will* store extra fields the way the pass ecosystem already does (password on the first line, `key: value` lines below, `otpauth://` lines as pass-otp does). *We won't* add a sidecar database or a field only paw can read.

### 4. Say what is not protected

Every design has gaps, and people can only choose well if they are told. Entry names, folders and the history of changes are visible to anyone who can read the repository. Old copies of the repository can be decrypted for ever if the key is ever compromised.

*So we will* state these limits in setup and in the README, in the same plain words as the benefits. *We won't* use "zero knowledge", "unbreakable" or "military grade".

### 5. Make convenience an explicit choice

Some conveniences weaken the model. The Face ID window is the main one: while it is open, entries already opened can be read with the phone and a face, without the key. That trade is the person's to make.

*So we will* show when the window is open and how long is left, let it be closed with one tap, limit it to entries the person opened with the key, and let it be turned off completely. *We won't* describe those entries as protected by the hardware key while the window is open.

### 6. Default to the safer setting

Most people keep the defaults, so the defaults should be the settings we would defend. Convenience that costs security or privacy is opt-in.

*So we will* require a touch for each decrypt, expire what we put on the clipboard and keep it off other devices, and make no network request other than to the git remote. Fetching website icons is off by default, because it tells the network which sites are in the vault.

### 7. Make setup finishable

The existing tools lose people at keys and sync. A newcomer with a new YubiKey should reach a working, backed-up vault without a laptop or a guide open in another tab.

*So we will* treat every setup step we cannot explain in two sentences as a design bug. *We won't* let setup finish until the person has either added a second key or backup, or said in so many words that losing this key means losing everything.

### 8. Say what happened and what to do

An error is part of the interface. It should name the problem in ordinary words, give the next step, and be exact about anything that can lock the person out.

*So we will* write "Wrong PIN. Two tries left before the key locks." *We won't* show a raw git message or a card status code as the only explanation.

### 9. Be pleasant, not austere

Security tools do not have to look like a terminal. paw should feel native, friendly and a little playful, and stay readable. Each visual signal means one thing.

*So we will* spend real effort on feel. *We won't* let decoration hide a security state or reuse the gold disc for anything other than the key.

### 10. Build in the open

All code is public, and so are the threat model and its known gaps. Reproducible builds are a goal. Today nobody can easily check that an App Store binary matches public source.

*So we will* publish pinned build instructions and report security issues openly. *We won't* claim that people can verify the App Store build until they really can.

## What paw protects and what it does not

Protected:

- The contents of each entry (passwords, notes, one-time-code secrets) against anyone who has only the repository, including the git host.
- The private key against a compromised phone, a stolen phone and a breached git host. It cannot be copied off the YubiKey.
- The vault against guessing: there is no master password to crack offline, and the key locks after three wrong PINs.
- Against phishing of a master secret: there is no password or sign-in page that unlocks the vault.
- Against bulk theft by malware: each decrypt needs a touch, so the whole vault cannot be decrypted silently.
- Against a vendor's breach, shutdown or price change: there is no vendor in the path.

Not protected:

- Entry names, folder names, how many entries there are, and when each was changed. These are plain text in the repository.
- Anything decrypted on a compromised phone. Malware can read what is opened, and can ask for a decrypt and wait for a touch.
- Entries inside the Face ID window, against someone who has the unlocked phone and can pass Face ID (including by force).
- Tampering by someone who can write to the repository. They cannot read entries, but they can swap, roll back or replace them, or add their own key to `.gpg-id` so that future entries are also encrypted to them.
- The future. There is no forward secrecy. A copy of the repository taken today can be decrypted later if the key is ever extracted, or if the key's algorithm is broken, including by a large quantum computer.
- Loss. One key and no backup means a lost or broken key ends access for good.
- Coercion. Someone who can force the PIN and a touch gets the entry.
- The app binary. People trust Apple's distribution and our build.

## Not goals

- A paw-run server, account, sync service or web vault.
- Recovery by us, in any form.
- Sharing, teams and organisation features beyond what pass already does with several recipients.
- Storing passkeys. They are not part of pass, and most exports leave them out.
- A new vault format.
- Being the right tool for everyone. Someone who cannot keep a second key or a backup is better served by a mainstream manager, and we should say so.

## Decisions so far

Made by the owner on 2026-11-22.

- **Face ID window:** on by default. Whether the device passcode may stand in for Face ID is still open (see below).
- **Face ID time is per entry:** each entry stays available for the chosen length from its own key touch, not until one shared end time.
- **PIN:** asked once while the key stays plugged in, and forgotten after 15 minutes with paw in the background. Locking the phone does not clear it.
- **Generate button:** the first version has a password generator on the add and edit forms.
- **Build order:** editing and push come before full AutoFill, as in [design.md](design.md).
- **Backup:** setup warns when there is no second key or backup, and does not block. A better backup story comes later.
- **Recipients:** paw warns whenever `.gpg-id` changes, and shows somewhere visible which keys can open the passwords.
- **Keys without a YubiKey:** possibly, later, for lower-security stores. Principle 1 would then read "the person chooses where the key lives, and paw says plainly what each choice protects".
- **Creating a key:** out of scope for now. paw assumes the YubiKey is already set up.
- **Creating a repository:** paw may ask for permission to create a private repository on the person's git host.
- **No remote:** a store that lives only on the device is supported.

## Open questions for the owner

1. May the device passcode stand in for Face ID during the window? The draft answer is no: someone who watched the passcode being typed should not get the opened entries.
2. How long is the Face ID window by default?
3. Should paw look keys up on a keyserver to show a name and email for each recipient? A lookup tells the keyserver which keys the person cares about, so the draft answer is: only when the person asks.
4. Should paw check signed commits or `.gpg-id.sig`?
5. Which remotes beyond GitHub come first?
