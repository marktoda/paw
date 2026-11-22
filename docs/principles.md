# paw principles

*First draft, 2026-11-22. Written for the owner to edit: change or delete anything that is not what you mean.*

paw is an iPhone and iPad app for people who keep their passwords in [pass](https://www.passwordstore.org/): one GPG-encrypted file per entry, in a git repository they host themselves. The key that decrypts lives on a YubiKey and nowhere else. paw is for people who would rather hold their own key than trust a vendor's servers, and who still want an app that is easy to set up and pleasant to use.

## Principles

### 1. Keep the key with the person

The private key is generated or stored on a hardware key the person holds, and decryption happens on that key. paw never sees the private key, so paw cannot leak it, back it up or recover it.

*So we won't* add a master password, a key stored in the app, or a "recover my vault" feature. Recovery means a second key or a backup the person made themselves.

### 2. Run no server

There is no paw account and no paw service. The app talks to the person's git remote and to nothing else unless they turn something on.

*So we won't* ship analytics, third-party crash reporting or a sync relay. If a feature needs a server run by us, the feature does not ship.

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

## Open questions for the owner

1. Is the Face ID window on or off by default, how long is it, and may the device passcode stand in for Face ID?
2. Do we require a second key or backup before setup finishes, or only warn?
3. Should paw warn when `.gpg-id` changes, and should it check signed commits or `.gpg-id.sig`?
4. Will paw ever support a key without a YubiKey (for example one held in the Secure Enclave)? That would change principle 1.
5. For newcomers: generate the key on the YubiKey (cannot be backed up, so two keys are needed) or generate it on the phone and load it onto two keys?
6. May paw ask for permission to create a private GitHub repository, or should the person create it and grant access to that one repository only?
7. Is "no remote at all" a supported way to use paw?
