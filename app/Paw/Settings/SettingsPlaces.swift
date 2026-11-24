import Foundation

extension AppModel {
    /// The open store's state directory, for the Settings screens. Setup
    /// clones the store into `<state>/repo` (core `setup::repo_dir`), on a
    /// device and in the simulator's fixture alike, so it is the
    /// repository's parent. `nil` for a store that can't be written.
    var settingsStateDir: String? {
        guard let repoDir = edits?.repoDir else { return nil }
        return URL(fileURLWithPath: repoDir).deletingLastPathComponent().path
    }
}

/// How Settings writes a key ID: the last 8 hex digits in two groups,
/// "9A1C 03F2", as design.md's "one ending 9A1C 03F2".
enum KeyIdWords {
    static func ending(_ id: String) -> String {
        let tail = String(id.suffix(8))
        guard tail.count == 8 else { return tail }
        return "\(tail.prefix(4)) \(tail.suffix(4))"
    }

    /// The whole ID in groups of four, for comparing with `gpg -k`.
    static func grouped(_ id: String) -> String {
        var groups: [String] = []
        var rest = Substring(id)
        while !rest.isEmpty {
            groups.append(String(rest.prefix(4)))
            rest = rest.dropFirst(4)
        }
        return groups.joined(separator: " ")
    }
}
