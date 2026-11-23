import Foundation

/// Where paw keeps the store on this iPhone: the setup state, the cloned
/// repository and the Face ID copies, all under one folder in Application
/// Support that device backups leave out ("Moving to a new iPhone",
/// design.md 2.12).
///
/// When the AutoFill extension needs the store, the folder moves to the App
/// Group container: only `base()` changes, to
/// `FileManager.default.containerURL(forSecurityApplicationGroupIdentifier:)`.
struct StoreLocation {
    /// The one folder everything below lives in.
    let root: URL

    static func base() throws -> URL {
        try FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
    }

    static func standard() throws -> StoreLocation {
        StoreLocation(root: try base().appendingPathComponent("paw", isDirectory: true))
    }

    /// The core's state directory: `setup`, `known_hosts`, `remote`,
    /// `enrolled-key`, `recipients` and the store itself in `repo/`.
    var stateDir: URL { root.appendingPathComponent("store", isDirectory: true) }

    /// The repository inside the state directory (`core/src/setup/mod.rs`).
    var repoDir: URL { stateDir.appendingPathComponent("repo", isDirectory: true) }

    /// The Face ID copies, kept apart from the store so the store can be
    /// set up again without touching them, and the other way round.
    var copiesDir: URL { root.appendingPathComponent("face-id", isDirectory: true) }

    /// Makes the folders and keeps them out of device backups.
    func prepare() throws {
        let manager = FileManager.default
        try manager.createDirectory(at: stateDir, withIntermediateDirectories: true)
        try manager.createDirectory(at: copiesDir, withIntermediateDirectories: true)
        var excluded = URLResourceValues()
        excluded.isExcludedFromBackup = true
        var folder = root
        try folder.setResourceValues(excluded)
    }

    /// "Remove this store from this iPhone": the downloaded store, its
    /// state and the Face ID copies' files.
    func removeAll() throws {
        let manager = FileManager.default
        for folder in [stateDir, copiesDir] where manager.fileExists(atPath: folder.path) {
            try manager.removeItem(at: folder)
        }
    }
}
