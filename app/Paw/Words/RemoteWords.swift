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
    case .portFallback(let fromHost, let fromPort, let host, let port):
        return "Port \(fromPort) of \(fromHost) could not be reached, so paw will use \(host) on port \(port) instead."
    }
}
