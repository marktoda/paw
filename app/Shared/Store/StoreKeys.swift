import Foundation

/// Which of paw's two processes has the store open: the app, or the
/// AutoFill extension. Each writes its own half of the key log in the
/// store's state directory.
enum StoreProcess: String, Sendable {
    case app
    case fill
}

/// The key's side of the store set up at a location, for one process on
/// its platform: a key port writing this process's key log, then, once
/// setup has enrolled a key, that key (any other is refused before a PIN
/// is sent) and the Face ID copies.
struct StoreKeys {
    let keyPort: KeyPort
    private let location: StoreLocation
    private let platform: Platform

    init(_ platform: Platform, at location: StoreLocation) {
        keyPort = platform.keyPort(keyLog: location.stateDir.path)
        self.location = location
        self.platform = platform
    }

    /// Loads the enrolled key into the key port, then makes the copies.
    func openEnrolled() throws -> CopyCache? {
        _ = try keyPort.loadEnrolledKey(stateDir: location.stateDir.path)
        return platform.copies(in: location.copiesDir)
    }
}
