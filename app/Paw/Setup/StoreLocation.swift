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

    /// Makes the folders, keeps them out of device backups and protects
    /// them at rest.
    func prepare() throws {
        let manager = FileManager.default
        try manager.createDirectory(at: stateDir, withIntermediateDirectories: true)
        try manager.createDirectory(at: copiesDir, withIntermediateDirectories: true)
        var excluded = URLResourceValues()
        excluded.isExcludedFromBackup = true
        var folder = root
        try folder.setResourceValues(excluded)
        protect()
    }

    /// Gives every folder and file under `root` (the Face ID copies, the
    /// pinned keys, the enrolled key, the known hosts, the cloned store)
    /// `NSFileProtectionComplete`: unreadable while the iPhone is locked.
    /// Set per item with `FileManager`, which needs no data-protection
    /// entitlement (the provisioning profile's entitlements are what
    /// `scripts/install.sh` signs with). A file created later in a
    /// protected folder takes the folder's class; this runs again when
    /// Home opens after setup, to catch any that didn't.
    ///
    /// Trade-off for later: background refresh (design 2.9) would need
    /// `repo/` readable while the phone is locked, i.e.
    /// `completeUntilFirstUserAuthentication` on the repository alone. Not
    /// needed yet, so `complete` everywhere.
    ///
    /// Returns how many items could not be protected.
    @discardableResult
    func protect() -> Int {
        let manager = FileManager.default
        var failures = 0
        for url in allItems() {
            do {
                try manager.setAttributes([.protectionKey: FileProtectionType.complete], ofItemAtPath: url.path)
            } catch {
                failures += 1
            }
        }
        return failures
    }

    /// "complete 42 · other 0 · unset 0": the protection classes under
    /// `root`, for the simulator's Settings readback.
    func protectionSummary() -> String {
        let manager = FileManager.default
        var complete = 0, other = 0, unset = 0
        for url in allItems() {
            switch (try? manager.attributesOfItem(atPath: url.path))?[.protectionKey] as? FileProtectionType {
            case .complete?: complete += 1
            case nil: unset += 1
            default: other += 1
            }
        }
        return "complete \(complete) · other \(other) · unset \(unset)"
    }

    private func allItems() -> [URL] {
        var items = [root]
        if let walker = FileManager.default.enumerator(at: root, includingPropertiesForKeys: nil) {
            for case let url as URL in walker {
                items.append(url)
            }
        }
        return items
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
