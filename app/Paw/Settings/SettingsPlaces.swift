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
