import Foundation

/// What the AutoFill screens read from: the store's names, the Face ID
/// copies and the key. Read-only on the store's folder.
struct AutoFillSource: @unchecked Sendable {
    let store: PassStore
    /// `nil` where Face ID copies cannot be kept: every fill uses the key.
    let copies: CopyCache?
    let keyPort: KeyPort
    let keys: any KeyConnector
    /// What a one-time code is made at.
    let clock: Clock
    /// The sites each name-only entry has filled on.
    let pairings: SitePairings
    let codes: OneTimeCodeEntries
}

/// The store as the extension finds it.
enum AutoFillOpening: @unchecked Sendable {
    case ready(AutoFillSource)
    /// Setup has not finished on this iPhone.
    case noStore
    /// paw held the store lock longer than the core waits (a save, a
    /// download). Nothing is wrong: trying again works.
    case busy
    /// There is a store, but it could not be read. AutoFill says only
    /// what to do: open paw, which says why.
    case unreadable
}

/// Opens the store paw set up, for the extension. Reads only: no `Setup`
/// (which creates folders), no sync, no GitHub, no network; what it writes
/// is listed at `StoreLocation`. What the core loads: one walk of the store's folder names (`PassStore`), the enrolled
/// key's identity (`enrolled-key`, a few lines), and, only when a list is
/// shown, the copies' small metadata files. Nothing is decrypted.
enum StoreForAutoFill {
    static func open() -> AutoFillOpening {
        do {
            let location = try StoreLocation.standard()
            guard location.isSetUp else { return .noStore }
            return .ready(try AutoFillSource.open(at: location, on: platform))
        } catch {
            return isStoreBusy(error) ? .busy : .unreadable
        }
    }

    /// The extension's hardware: in the simulator's debug builds the
    /// launch's simulated key on the system clock, with no Face ID copies
    /// (the simulator has no Secure Enclave the two processes share).
    private static var platform: Platform {
        #if DEBUG && targetEnvironment(simulator)
        .simulated(.fill, keys: .launched, clock: SystemClock(), keystore: nil)
        #else
        .device(.fill)
        #endif
    }
}

extension AutoFillSource {
    /// Everything the AutoFill screens read, for the store set up at
    /// `location`.
    static func open(at location: StoreLocation, on platform: Platform) throws -> AutoFillSource {
        let stateDir = location.stateDir.path
        let store = try PassStore(dir: location.repoDir.path)
        let keys = StoreKeys(platform, at: location)
        return AutoFillSource(
            store: store, copies: try keys.openEnrolled(), keyPort: keys.keyPort, keys: platform.keys,
            clock: platform.clock, pairings: SitePairings.open(stateDir: stateDir),
            codes: OneTimeCodeEntries.open(stateDir: stateDir))
    }
}
