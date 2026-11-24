import Foundation

/// Where paw keeps the store on this iPhone: the setup state, the cloned
/// repository and the Face ID copies, all under one folder in the App Group
/// container that paw and its AutoFill extension share, left out of device
/// backups ("Moving to a new iPhone", design.md 2.12).
///
/// The app writes here. The extension only reads the store (see
/// `StoreForAutoFill`); what it writes is a Face ID copy in `copiesDir`
/// and `site-pairings.json` in `stateDir`, each written by rename so two
/// processes are safe.
struct StoreLocation {
    /// The App Group both targets are signed with.
    static let appGroup = "group.com.marktoda.paw"

    /// The one folder everything below lives in.
    let root: URL

    enum Problem: Error, CustomStringConvertible {
        case noAppGroup

        var description: String {
            "paw can't reach its shared folder (\(StoreLocation.appGroup)). This build may not be signed for it."
        }
    }

    /// The App Group container. The unsigned simulator build has none, so
    /// there it is a folder in the app's Application Support, apart from
    /// where paw kept the store before (`LegacyStore`); the debug AutoFill
    /// harness runs inside the app and so shares it.
    static func base() throws -> URL {
        if let group = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup) {
            return group
        }
        #if targetEnvironment(simulator)
        return try FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            .appendingPathComponent("app-group", isDirectory: true)
        #else
        throw Problem.noAppGroup
        #endif
    }

    static func standard() throws -> StoreLocation {
        StoreLocation(root: try base().appendingPathComponent("paw", isDirectory: true))
    }

    /// Setup finished and the store is on this iPhone. Reads two files and
    /// writes nothing, so the extension can ask (`Setup.open` would create
    /// folders and may restart a download).
    var isSetUp: Bool {
        let setup = stateDir.appendingPathComponent("setup")
        guard let text = try? String(contentsOf: setup, encoding: .utf8),
              text.split(separator: "\n").first == "step=done" else { return false }
        return FileManager.default.fileExists(atPath: repoDir.appendingPathComponent(".gpg-id").path)
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

    /// "items 42 · refused 0 · complete 42 · other 0 · unset 0": every
    /// item under `root`, how many refused `.complete` when set again now,
    /// and the classes read back, for the simulator's Settings readback.
    /// The simulator's file system accepts the class but does not record
    /// it, so there every item reads back unset; a device records it.
    func protectionSummary() -> String {
        let refused = protect()
        let manager = FileManager.default
        var complete = 0, other = 0, unset = 0
        for url in allItems() {
            switch (try? manager.attributesOfItem(atPath: url.path))?[.protectionKey] as? FileProtectionType {
            case .complete?: complete += 1
            case nil: unset += 1
            default: other += 1
            }
        }
        return "items \(complete + other + unset) · refused \(refused) · complete \(complete) · other \(other) · unset \(unset)"
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
