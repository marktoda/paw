import Foundation

#if DEBUG && targetEnvironment(simulator)
/// The fixture store, set up the way a real one is so it can be written:
/// the core's simulated git host serves it, and setup runs to the end
/// against it (read the key, check the host, download, open one entry),
/// which pins the key's public half and accepts every `.gpg-id`, with key
/// tasks on a key port of its own, as the app runs them. Saving
/// then needs no key and no network, as on a real store, and each change
/// is an unpushed commit on top of what was downloaded.
///
/// Setup uses its own simulated key with the same key material and no
/// touch, so the key the UI tests drive keeps its PIN tries and touches.
///
/// `-pawSimRecipients changed` then has another computer give `work/` its
/// own `.gpg-id` naming the same key, and paw downloads it: saving in
/// `work` asks for a review first. `-pawSimRecipients missing` adds a
/// second key paw has no copy of to it as well: after the review, paw
/// can't save there.
enum WritableFixture {
    struct Prepared {
        let stateDir: String
        let repoDir: String
        let writer: StoreWriter
        let sync: StoreSync?
        /// Serves the store while the app runs; `SimRemoteHandle` keeps it.
        let host: SimRemote
    }

    private struct Failure: LocalizedError {
        let errorDescription: String?
    }

    /// The tasks are followed by polling their state (`finish`).
    private final class Unheard: KeyTaskListener, @unchecked Sendable {
        func stateChanged(state: KeyTaskState) {}
    }

    /// Runs a key task to its end on this thread, as the key sheet would,
    /// typing `pin` whenever it asks. The key needs no touch.
    private static func finish(_ task: KeyTask, pin: String, _ what: String) throws {
        let deadline = Date().addingTimeInterval(60)
        while Date() < deadline {
            switch task.state() {
            case .pin:
                task.supplyPin(pin: Data(pin.utf8))
            case .done:
                return
            case .failed(let error):
                throw Failure(errorDescription: "\(what) failed: \(error).")
            case .syncFailed(let error):
                throw Failure(errorDescription: "\(what) failed: \(error).")
            case .setupFailed(let error):
                throw Failure(errorDescription: "\(what) failed: \(error).")
            case .cancelled:
                throw Failure(errorDescription: "\(what) was cancelled.")
            default:
                break
            }
            Thread.sleep(forTimeInterval: 0.01)
        }
        task.cancel()
        throw Failure(errorDescription: "\(what) did not finish.")
    }

    static func prepare(root: URL) throws -> Prepared {
        let stateDir = root.appendingPathComponent("state", isDirectory: true)
        try FileManager.default.createDirectory(at: stateDir, withIntermediateDirectories: true)
        let pin = "123456"
        let key = SimulatedKey(pin: pin, touchRequired: false, touchTimeoutMs: 1_000)
        let connection = key.asConnection()
        let host = try SimRemote.start(key: key)
        // Owned by the handle alone, which the sync screens' debug controls
        // drive (`Sync/SimRemoteControls.swift`).
        SimRemoteHandle.shared.register(host, key: key)
        // Its own key port, as the app has one: the PIN is typed once.
        let port = KeyPort(clock: SystemClock())
        let session = try port.pluggedIn(connection: connection)
        let setup = try Setup.open(stateDir: stateDir.path)
        var state = try setup.readKeyFromSession(session: session)
        state = try setup.setAddress(address: host.address())
        state = try setup.checkHost()
        if case .confirmHost(_, _, let fingerprint) = state {
            state = try setup.confirmHost(fingerprint: fingerprint)
        }
        try finish(setup.downloadWithKey(port: port, listener: Unheard()), pin: pin, "The download")
        try finish(setup.testDecryptWithKey(port: port, listener: Unheard()), pin: pin, "The key check")
        state = setup.state()
        guard case .done = state else {
            throw Failure(errorDescription: "Setup stopped at \(state).")
        }
        let sync = try? StoreSync.open(stateDir: stateDir.path)
        if let mode = LaunchArguments.string("pawSimRecipients"), let sync {
            var lines = key.fingerprintHex() + "\n"
            if mode == "missing" {
                lines += SimulatedKey.newOther(pin: pin, touchRequired: false, touchTimeoutMs: 1_000).fingerprintHex() + "\n"
            }
            _ = try host.commitFile(
                path: "work/.gpg-id", contents: Data(lines.utf8), message: "Give work its own list of keys.")
            try finish(sync.refreshWithKey(port: port, listener: Unheard()), pin: pin, "The refresh")
        }
        return Prepared(
            stateDir: stateDir.path, repoDir: storeRepoDir(stateDir: stateDir.path),
            writer: StoreWriter.open(stateDir: stateDir.path), sync: sync, host: host)
    }

    /// Gives `model` its write side, or says on Home why it has none.
    @MainActor
    static func attach(_ prepared: Result<Prepared, Error>, to model: AppModel) {
        switch prepared {
        case .success(let prepared):
            model.edits = StoreEdits(
                writer: prepared.writer, sync: prepared.sync, stateDir: prepared.stateDir, sendingNote: nil)
        case .failure(let error):
            model.notice = Notice(
                problem: "Adding passwords isn't available in this test store.", nextStep: describe(error))
        }
    }
}
#endif
