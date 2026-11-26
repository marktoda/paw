import Foundation

#if DEBUG && targetEnvironment(simulator)
/// The core's fixture store in a fresh temporary folder, opened as a
/// `StoreSession` on simulated hardware: a simulated keystore that answers
/// as Face ID does, and one simulated clock for the copies, the key port
/// and the clipboard, so moving it on ends copies and, in the background,
/// the held PIN. `-pawFixture 1` (`FixtureLaunch`), the gallery and the
/// unit tests' `Fixture` all open the store here.
///
/// `writable` sets the store up the way a real one is, so it can be
/// written: the core's simulated git host serves it and setup runs to the
/// end against it (`SimRemote.setUpStore`), which pins the key's public
/// half and accepts every `.gpg-id`. Saving then needs no key and no
/// network, as on a real store, and each change is an unpushed commit on
/// top of what was downloaded. Setup uses a simulated key of its own with
/// the same key material and no touch, so the key the screens use keeps
/// its PIN tries and touches.
///
/// `-pawSimRecipients changed` then has another computer give `work/` its
/// own `.gpg-id` naming the same key, and paw downloads it: saving in
/// `work` asks for a review first. `-pawSimRecipients missing` adds a
/// second key paw has no copy of to it as well: after the review, paw
/// can't save there.
struct FixtureSession {
    /// The PIN of every simulated key the fixture makes.
    static let pin = "123456"

    /// Where the key the screens use comes from.
    enum Key {
        /// The launched app's simulated key (`-pawSimPin`, `-pawSimTouch`,
        /// `-pawSimUnplugged`, …), through `KeyConnection`.
        case launched
        /// A key of the caller's own, read through `connect`.
        case own(SimulatedKey, connect: KeyReader.Connect)
    }

    let session: StoreSession
    let clock: SimClock
    let keystore: SimKeystore

    /// `keyLog` writes the key log into the fixture's own folder, so
    /// Settings › Key diagnostics shows it. The launched key goes with the
    /// launch's clock and Face ID (`SimulatedClockHolder`,
    /// `SimulatedKeystoreHolder`); a key of the caller's own gets a clock
    /// and a keystore of its own, answering as `faceId`.
    @MainActor
    init(key: Key, writable: Bool, faceId: SimFaceId = .match, keyLog: Bool = false) throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("paw-fixture-\(UUID().uuidString)", isDirectory: true)
        let copiesDir = root.appendingPathComponent("window", isDirectory: true)
        try FileManager.default.createDirectory(at: copiesDir, withIntermediateDirectories: true)
        let repoDir: String
        let writing: StoreSession.Writing?
        if writable {
            let stateDir = root.appendingPathComponent("state", isDirectory: true).path
            writing = try Self.setUp(stateDir: stateDir)
            repoDir = storeRepoDir(stateDir: stateDir)
        } else {
            repoDir = root.appendingPathComponent("store", isDirectory: true).path
            try writeFixtureStore(dir: repoDir)
            writing = nil
        }

        let keystore: SimKeystore
        let clock: SimClock
        let connect: KeyReader.Connect
        switch key {
        case .launched:
            keystore = SimulatedKeystoreHolder.keystore
            clock = SimulatedClockHolder.shared.clock
            connect = { try KeyConnection.open() }
        case .own(_, let ownConnect):
            keystore = SimKeystore()
            keystore.setFaceId(answer: faceId)
            clock = SimClock(wallMs: Int64(Date().timeIntervalSince1970 * 1000))
            connect = ownConnect
        }
        let shared = clock.asClock()
        let copies = CopyCache(dir: copiesDir.path, keystore: keystore.asKeystore(), clock: shared)
        let keyPort = KeyPort(clock: shared)
        if keyLog {
            // Before the key is first read.
            keyPort.setKeyLog(stateDir: root.path, process: "app")
        }
        session = try StoreSession(StoreSession.Parts(
            repoDir: repoDir, copies: copies, clock: shared, keyPort: keyPort, connect: connect,
            writing: writing, gitHub: nil, setUp: nil))
        self.clock = clock
        self.keystore = keystore
    }

    /// Sets the store up in `stateDir` from a simulated git host.
    private static func setUp(stateDir: String) throws -> StoreSession.Writing {
        try FileManager.default.createDirectory(atPath: stateDir, withIntermediateDirectories: true)
        let key = SimulatedKey(pin: pin, touchRequired: false, touchTimeoutMs: 1_000)
        let host = try SimRemote.start(key: key)
        // Owned by the handle alone, which the sync screens' debug controls
        // drive (`SimRemoteControls.swift`).
        SimRemoteHandle.shared.register(host, key: key)
        try host.setUpStore(key: key, pin: pin, stateDir: stateDir)
        let sync = try? StoreSync.open(stateDir: stateDir)
        if let mode = LaunchArgument.simRecipients.string, sync != nil {
            try host.giveWorkItsOwnKeys(key: key, pin: pin, stateDir: stateDir, missingKey: mode == "missing")
        }
        // Only now: setup's own download must not be refused or cut off.
        SimRemoteHandle.shared.storeReady(repoDir: storeRepoDir(stateDir: stateDir))
        return StoreSession.Writing(writer: StoreWriter.open(stateDir: stateDir), sync: sync, stateDir: stateDir)
    }
}
#endif
