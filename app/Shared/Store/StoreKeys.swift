import Foundation

/// Which of paw's two processes has the store open: the app, or the
/// AutoFill extension. Each writes its own half of the key log in the
/// store's state directory.
enum StoreProcess: String, Sendable {
    case app
    case fill
}

/// The key's side of the store set up at a location, for one process: a
/// key port writing this process's key log, then, once setup has enrolled
/// a key, that key (any other is refused before a PIN is sent) and the
/// Face ID copies.
struct StoreKeys {
    let keyPort: KeyPort
    private let location: StoreLocation
    private let makeCopies: (URL) -> CopyCache?

    /// `copies` makes the Face ID copies in the copies folder: the
    /// platform's for `process` (`PlatformCopies`) unless given.
    init(_ process: StoreProcess, at location: StoreLocation, clock: Clock, copies: ((URL) -> CopyCache?)? = nil) {
        keyPort = KeyPort(clock: clock)
        keyPort.setKeyLog(stateDir: location.stateDir.path, process: process.rawValue)
        self.location = location
        if let copies {
            makeCopies = copies
        } else {
            makeCopies = { dir in PlatformCopies.make(dir: dir.path, clock: clock, process: process) }
        }
    }

    /// Loads the enrolled key into the key port, then makes the copies.
    func openEnrolled() throws -> CopyCache? {
        _ = try keyPort.loadEnrolledKey(stateDir: location.stateDir.path)
        return makeCopies(location.copiesDir)
    }
}
