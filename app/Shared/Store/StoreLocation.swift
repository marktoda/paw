import Foundation

/// Where paw keeps the store on this iPhone: the setup state, the cloned
/// repository and the Face ID copies, all under one folder in the App Group
/// container that paw and its AutoFill extension share, left out of device
/// backups ("Moving to a new iPhone", design.md 2.12).
///
/// The app writes here. The extension only reads the store (see
/// `StoreForAutoFill`); what it writes is a Face ID copy in `copiesDir`
/// and `site-pairings.json` and `one-time-codes.json` in `stateDir`, each
/// written by rename so two processes are safe.
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

    /// Setup finished and the store is on this iPhone. Writes nothing, so
    /// the extension can ask (`Setup.open` would create folders and may
    /// restart a download).
    var isSetUp: Bool {
        storeIsSetUp(stateDir: stateDir.path)
    }

    /// The core's state directory: its files and the store itself.
    var stateDir: URL { root.appendingPathComponent("store", isDirectory: true) }

    var repoDir: URL { URL(fileURLWithPath: storeRepoDir(stateDir: stateDir.path), isDirectory: true) }

    /// The Face ID copies, kept apart from the store so the store can be
    /// set up again without touching them, and the other way round.
    var copiesDir: URL { root.appendingPathComponent("face-id", isDirectory: true) }
}
