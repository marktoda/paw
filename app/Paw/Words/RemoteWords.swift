import Foundation

/// Why paw won't check out a path, as one phrase: "it is a symbolic link".
func refusedWords(_ refused: RefusedPath) -> String {
    refused.reasons.map(refusedReasonWords).joined(separator: ", and ")
}

func refusedReasonWords(_ reason: RefusedReason) -> String {
    switch reason {
    case .tooBig(let maxEntries, let maxMib):
        "the repository holds more than \(maxEntries) files and folders, or more than \(maxMib) MiB of names"
    case .notUtf8: "its name is not UTF-8"
    case .hasSlash: "its name contains a slash"
    case .pathTooLong: "its path is too long"
    case .unsafeName(let part): unsafePartWords(part)
    case .gpgIdAlias: "some file systems would take it for .gpg-id"
    case .gpgIdNotFile: "a .gpg-id must be a file"
    case .nestedTooDeep: "its folders are nested too deeply"
    case .symbolicLink: "it is a symbolic link"
    case .submodule: "it is a submodule, a link to another repository"
    case .notRegularFile: "it is not a regular file"
    case .appearsTwice: "it appears twice in the same folder"
    case .sameNameAs(let first): "on an iPhone it is the same name as \(first)"
    }
}

func unsafePartWords(_ part: UnsafePart) -> String {
    switch part {
    case .nulOrBackslash: "it contains a NUL or a backslash"
    case .absolute: "it is absolute"
    case .emptyComponent: "it has an empty component"
    case .dotComponent: "it has a `.` or `..` component"
    case .gitDirectory: "it names git's own directory"
    }
}

/// Why an address can't be used, and what to type instead.
func addressProblemWords(_ problem: AddressProblem) -> String {
    switch problem {
    case .empty:
        "Type or paste the address of your password store."
    case .hasSpaces:
        "An address has no spaces in it. Check what was pasted."
    case .notGit:
        "That doesn't look like a git address. It looks like git@github.com:you/password-store.git."
    case .useSsh:
        "paw connects over SSH for now. Use the address that starts with git@ or ssh://."
    case .hasSecret:
        "That address has a user name, password or token in it. paw logs in with your YubiKey and needs none of "
            + "them: remove that part. If it was a password or token, change it, since it has been copied."
    case .needsSshAddress(let host):
        "paw needs the SSH address, as in git@\(host):you/password-store.git. Your git host shows it beside the "
            + "Clone button."
    case .looksLikeFolder:
        "That looks like a folder on a computer. paw needs the address of a git server."
    case .noUser:
        "Add the user name in front, as in git@github.com:you/password-store.git."
    case .noRepository:
        "Add the repository after the colon, as in git@github.com:you/password-store.git."
    case .sshWithoutUser:
        "Add the user name, as in ssh://git@example.com/you/password-store.git."
    case .badPort:
        "The port after the colon must be a number from 1 to 65535."
    case .sshWithoutPath:
        "Add the repository's path after the host, as in ssh://git@example.com/you/password-store.git."
    case .notARepositoryPage:
        "Use the address of the repository itself, as in https://github.com/you/password-store, or its SSH address."
    }
}

/// How paw changed the address, under the field.
func addressNoticeWords(_ notice: AddressNotice) -> String {
    let instead = { (address: String) in "paw connects over SSH, so it will use \(address) instead." }
    switch notice {
    case .gitHubShortForm(let owner, let repo, let address):
        return "That was GitHub's short form for \(owner)/\(repo). " + instead(address)
    case .gitHubWithoutUser(let address):
        return "That was a GitHub address without its user name. " + instead(address)
    case .gitHubWebAddress(let address):
        return "That was a web address. " + instead(address)
    case .portFallback(let fallback):
        return "Port \(fallback.fromPort) of \(fallback.fromHost) could not be reached, so paw will use "
            + "\(fallback.host) on port \(fallback.port) instead."
    }
}

/// "the remote contains something paw will not check out: up (it is a
/// symbolic link)", naming the first five paths.
func refusedMessage(_ paths: [RefusedPath]) -> String {
    let named = paths.prefix(namedAtMost).map { "\($0.path) (\(refusedWords($0)))" }
    var message = "the remote contains something paw will not check out: " + named.joined(separator: "; ")
    if let more = andMore(paths.count) {
        message += "; " + more
    }
    return message
}

/// What went wrong underneath a step, as one line under Details.
func faultDetail(_ fault: Fault) -> String {
    switch fault {
    case .storeBusy: StoreBusyWords.line
    case .remote(let failure): remoteDetail(failure)
    case .refused(let paths): "not a safe name for an entry: " + refusedMessage(paths)
    case .local(let detail): detail
    }
}

/// What went wrong with the remote, as one line under Details, or as the
/// sentence where a screen has room for one line only. The same words in
/// every workflow; screens with words of their own (setup's, sync's, the
/// GitHub picker's) use those first.
func remoteDetail(_ failure: RemoteFailure) -> String {
    switch failure {
    case .noConnection(_, let detail, _), .unreachable(_, let detail):
        "could not reach the git host: \(detail)"
    case .hostKeyChanged(let host, let expected, let presented, _):
        "\(host)'s key changed: expected \(expected.joined(separator: " or ")), it presented \(presented)"
    case .hostKeyNotConfirmed(let host):
        "\(host) now shows a different key from the one you checked"
    case .noSavedHostKey(let host):
        "\(host) has no saved key; set the store up again"
    case .hostKeyBundled(let host):
        hostKeyBundled(host)
    case .keyNotAccepted:
        "the git host did not accept this key"
    case .loginKeyUnusable(let detail):
        "this key cannot log in to a git host: \(detail)"
    case .repositoryMissing(_, _, _, let detail), .pushRefused(let detail):
        detail
    case .refusedContent(let paths):
        refusedMessage(paths)
    case .keptMoving:
        "the remote kept changing; try again"
    case .notSignedIn:
        "not signed in to GitHub"
    case .signedOut:
        "GitHub sign-in ended; sign in again"
    case .unauthorised:
        // Met here when reading the store's repository; the picker words
        // it for the list (`GitHubWords`).
        "GitHub: GitHub did not let paw's sign-in read this repository; is paw still installed on it?"
    case .notLinked:
        "no GitHub repository is chosen for this store"
    case .wrongRepository(let store, let granted):
        "this store is \(store), but GitHub sign-in is for \(granted); paw did not use it"
    case .gitHubRefused(let message):
        "GitHub: \(message)"
    }
}

/// Whether the GitHub sign-in has ended: sign in again, or go on with the
/// key.
func signInEnded(_ failure: RemoteFailure) -> Bool {
    switch failure {
    case .notSignedIn, .signedOut: true
    default: false
    }
}

/// A pinned host presented another key: "github.com's identity changed".
func hostIdentityChanged(_ host: String) -> String {
    "\(host)'s identity changed"
}

/// The details under `hostIdentityChanged`: the keys it was expected to
/// present, and the one it did.
enum HostKeyWords {
    static func expected(_ keys: String) -> String {
        "Expected \(keys)"
    }

    static func presented(_ key: String) -> String {
        "It presented \(key)"
    }
}

/// Why a host's key can't be forgotten or replaced: paw ships with it.
func hostKeyBundled(_ host: String) -> String {
    "\(host)'s keys come with paw and can't be forgotten. A different key means this connection isn't reaching "
        + "the real \(host): try another network"
}
