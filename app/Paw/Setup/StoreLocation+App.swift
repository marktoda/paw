import Foundation

extension StoreLocation {
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

    #if DEBUG && targetEnvironment(simulator)
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
    #endif

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
