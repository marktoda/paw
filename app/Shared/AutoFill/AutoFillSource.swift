import Foundation

/// What the AutoFill screens read from: the store's names, the Face ID
/// copies and the key. Read-only on the store's folder.
struct AutoFillSource: @unchecked Sendable {
    let store: PassStore
    /// `nil` where Face ID copies cannot be kept: every fill uses the key.
    let copies: CopyCache?
    let keyPort: KeyPort
    let connect: @Sendable () throws -> CardConnection
    /// What a one-time code is made at.
    let clock: Clock
    /// The sites each name-only entry has filled on.
    let pairings: SitePairings
    /// Which entries are known to have a one-time code.
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
    /// There is a store, but it could not be read. The raw reason, for no
    /// one but a debugger: the screen says only what to do.
    case unreadable(String)
}

/// Opens the store paw set up, for the extension. Reads only: no `Setup`
/// (which creates folders), no sync, no GitHub, no network. The files
/// it writes besides a Face ID copy are `site-pairings.json`, after a
/// confirmed fill by name, and `one-time-codes.json`, whether an entry it
/// filled has a code, while the owner has that list on (each written by
/// rename, safe for two processes). What the core
/// loads: one walk of the store's folder names (`PassStore`), the enrolled
/// key's identity (`enrolled-key`, a few lines), and, only when a list is
/// shown, the copies' small metadata files. Nothing is decrypted.
enum StoreForAutoFill {
    static func open() -> AutoFillOpening {
        do {
            let location = try StoreLocation.standard()
            guard location.isSetUp else { return .noStore }
            return .ready(try AutoFillSource.open(
                at: location, clock: SystemClock(), connect: { try KeyConnection.open() }))
        } catch {
            return isStoreBusy(error) ? .busy : .unreadable(String(describing: error))
        }
    }
}

extension AutoFillSource {
    /// Everything the AutoFill screens read, for the store set up at
    /// `location`, in this order: the store's folder names, the extension's
    /// key side (`StoreKeys`; `copies` as there), the site pairings and the
    /// list of entries with a code.
    static func open(
        at location: StoreLocation, clock: Clock, connect: @escaping KeyReader.Connect,
        copies: ((URL) -> CopyCache?)? = nil
    ) throws -> AutoFillSource {
        let stateDir = location.stateDir.path
        let store = try PassStore(dir: location.repoDir.path)
        let keys = StoreKeys(.fill, at: location, clock: clock, copies: copies)
        return AutoFillSource(
            store: store, copies: try keys.openEnrolled(), keyPort: keys.keyPort, connect: connect, clock: clock,
            pairings: SitePairings.open(stateDir: stateDir),
            codes: OneTimeCodeEntries.open(stateDir: stateDir))
    }
}
