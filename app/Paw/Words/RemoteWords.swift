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
