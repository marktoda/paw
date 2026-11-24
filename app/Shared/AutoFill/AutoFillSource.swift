import Foundation

/// What the AutoFill screens read from: the store's names, the Face ID
/// copies and the key. Read-only on the store's folder.
struct AutoFillSource: @unchecked Sendable {
    let store: PassStore
    /// `nil` where Face ID copies cannot be kept: every fill uses the key.
    let copies: CopyCache?
    let keyPort: KeyPort
    let connect: @Sendable () throws -> CardConnection
    /// The sites each name-only entry has filled on. `nil` keeps none:
    /// every name match then asks.
    var pairings: SitePairings? = nil
}

/// The store as the extension finds it.
enum AutoFillOpening: @unchecked Sendable {
    case ready(AutoFillSource)
    /// Setup has not finished on this iPhone.
    case noStore
    /// paw held the store lock longer than the core waits (a save, a
    /// download). Nothing is wrong: trying again works.
    case busy
    /// There is a store, but it could not be read. The raw reason, for no
    /// one but a debugger: the screen says only what to do.
    case unreadable(String)
}

/// How a request ends without a fill. Mirrors the `ASExtensionError` codes
/// the extension passes on, without importing AuthenticationServices here.
enum AutoFillEnd: String, Sendable {
    case userCanceled
    case userInteractionRequired
    case credentialIdentityNotFound
    case failed
}

/// Opens the store paw set up, for the extension. Reads only: no `Setup`
/// (which creates folders), no sync, no GitHub, no network. The one file
/// it writes besides a Face ID copy is `site-pairings.json`, after a
/// confirmed fill by name (written by rename, safe for two processes). What the core
/// loads: one walk of the store's folder names (`PassStore`), the enrolled
/// key's identity (`enrolled-key`, a few lines), and, only when a list is
/// shown, the copies' small metadata files. Nothing is decrypted.
enum StoreForAutoFill {
    static func open() -> AutoFillOpening {
        guard let location = try? StoreLocation.standard(), location.isSetUp else {
            return .noStore
        }
        do {
            let store = try PassStore(dir: location.repoDir.path)
            let keyPort = KeyPort(clock: SystemClock())
            // The store's key: any other key is refused before a PIN is sent.
            _ = try keyPort.loadEnrolledKey(stateDir: location.stateDir.path)
            #if targetEnvironment(simulator)
            // The simulator has no Secure Enclave the two processes share.
            let copies: CopyCache? = nil
            #else
            let copies = PlatformCopies.make(dir: location.copiesDir.path)
            #endif
            return .ready(AutoFillSource(
                store: store, copies: copies, keyPort: keyPort,
                connect: { try KeyConnection.openForSession() },
                pairings: SitePairings.open(stateDir: location.stateDir.path)))
        } catch {
            let detail = String(describing: error)
            return isStoreBusy(message: detail) ? .busy : .unreadable(detail)
        }
    }

    /// What `provideCredentialWithoutUserInteraction` answers: always a
    /// screen. Every copy is sealed behind Face ID, which needs one, so a
    /// fill without one is never possible.
    static let answerWithoutScreen = AutoFillEnd.userInteractionRequired
}
