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
    static let pin = SimulatedKeyConnector.defaultPin

    let session: StoreSession

    /// Simulated hardware of a caller's own: `key`, always in, whose first
    /// `failingReads` reads fail, and a clock and a keystore of its own,
    /// answering as `faceId`.
    struct OwnHardware {
        let platform: Platform
        let keys: SimulatedKeyConnector
        let clock: SimClock
        let keystore: SimKeystore

        @MainActor
        init(key: SimulatedKey, failingReads: Int = 0, faceId: SimFaceId = .match) {
            keys = SimulatedKeyConnector(key: key, attachFailures: failingReads)
            clock = SimClock(wallMs: Int64(Date().timeIntervalSince1970 * 1000))
            keystore = SimKeystore()
            keystore.setFaceId(answer: faceId)
            platform = .simulated(.app, keys: keys, clock: clock.asClock(), keystore: keystore.asKeystore())
        }
    }

    /// The fixture on `platform`. `keyLog` writes the key log into the
    /// fixture's own folder, so Settings › Key diagnostics shows it.
    @MainActor
    init(on platform: Platform, writable: Bool, keyLog: Bool = false) throws {
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
        // Settings' AutoFill sections work on the fixture's own state; its
        // entries never reach the system's suggestions.
        let autoFill = writing.map { writing in
            AutoFillSuggestions(stateDir: writing.stateDir, submit: { _ in Task {} })
        }
        session = try StoreSession(StoreSession.Parts(
            repoDir: repoDir, copies: platform.copies(in: copiesDir), clock: platform.clock,
            keyPort: platform.keyPort(keyLog: keyLog ? root.path : nil), keys: platform.keys,
            writing: writing, gitHub: nil, autoFill: autoFill, setUp: nil))
    }

    private static func setUp(stateDir: String) throws -> StoreSession.Writing {
        try FileManager.default.createDirectory(atPath: stateDir, withIntermediateDirectories: true)
        let key = SimulatedKey(pin: pin, touchRequired: false, touchTimeoutMs: 1_000)
        let host = try SimRemote.start(key: key)
        // Owned by the handle alone, which the sync screens' debug controls
        // drive (`SimRemoteControls.swift`).
        SimRemoteHandle.shared.register(host, key: key)
        try host.setUpStore(key: key, pin: pin, stateDir: stateDir)
        let sync = Result { try StoreSync.open(stateDir: stateDir) }
        if let mode = LaunchArgument.simRecipients.string, case .success = sync {
            try host.giveWorkItsOwnKeys(key: key, pin: pin, stateDir: stateDir, missingKey: mode == "missing")
        }
        if LaunchArgument.simPairings.bool {
            SimulatedSitePairings.seed(stateDir: stateDir)
        }
        // Only now: setup's own download must not be refused or cut off.
        SimRemoteHandle.shared.storeReady(repoDir: storeRepoDir(stateDir: stateDir))
        return StoreSession.Writing(writer: StoreWriter.open(stateDir: stateDir), sync: sync, stateDir: stateDir)
    }
}

/// Bringing the fixture to a state as taps would, for the gallery and the
/// unit tests.
@MainActor
extension FixtureSession {
    struct Stuck: LocalizedError {
        let errorDescription: String?
    }

    /// A tap on the entry's row, the PIN typed if the key asks for it, and
    /// the sheet gone at Done: the entry is on top of the stack.
    static func open(_ path: String, in model: AppModel) async throws {
        model.open(EntryRef(path: path))
        var typed = false
        try await waitUntil("\(path) to open") {
            switch model.keyTask.stage {
            case .pin where !typed:
                typed = true
                model.keyTask.pin = pin
                model.keyTask.continueWithPin()
            case .done:
                return true
            default:
                break
            }
            return false
        }
        model.cancelKeyTask()
        model.keySheetDismissed()
    }

    /// A new entry with a typed password, saved from the add form as its
    /// Save does once it knows who can open the folder.
    static func add(_ name: String, folder: String, in model: AppModel) async throws {
        guard let edits = model.storeSession.edits else {
            throw Stuck(errorDescription: "The fixture store can't be written.")
        }
        let form = edits.addForm(folder: folder)
        form.name = name
        form.password = "Typed-pass-42"
        form.changed()
        try await waitUntil("who can open \(folder)") { form.folderWritable != nil }
        guard form.canSave else {
            throw Stuck(errorDescription: "\(name) can't be saved: \(form.problems)")
        }
        guard edits.save(form) else {
            throw Stuck(errorDescription: "\(name) wasn't saved: \(String(describing: form.saveProblem))")
        }
    }

    static func waitUntil(_ what: String, timeout: TimeInterval = 30, _ condition: () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() {
            if Date() > deadline {
                throw Stuck(errorDescription: "Waited \(Int(timeout)) s for \(what).")
            }
            try await Task.sleep(for: .milliseconds(10))
        }
    }
}
#endif
