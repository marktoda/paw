import Foundation

#if targetEnvironment(simulator)
/// The fixture store, set up the way a real one is so it can be written:
/// the core's simulated git host serves it, and setup runs to the end
/// against it (read the key, check the host, download, open one entry),
/// which pins the key's public half and accepts every `.gpg-id`. Saving
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
        /// Serves the store while the app runs.
        let host: SimRemote
    }

    private struct Failure: LocalizedError {
        let errorDescription: String?
    }

    private final class Quiet: ProgressListener, @unchecked Sendable {
        func progress(received: UInt32, total: UInt32) -> Bool { true }
    }

    static func prepare(root: URL) throws -> Prepared {
        let stateDir = root.appendingPathComponent("state", isDirectory: true)
        try FileManager.default.createDirectory(at: stateDir, withIntermediateDirectories: true)
        let pin = "123456"
        let key = SimulatedKey(pin: pin, touchRequired: false, touchTimeoutMs: 1_000)
        let connection = key.asConnection()
        let host = try SimRemote.start(key: key)
        let setup = try Setup.open(stateDir: stateDir.path)
        var state = try setup.readKey(connection: connection)
        state = try setup.setAddress(address: host.address())
        state = try setup.checkHost()
        if case .confirmHost(_, _, let fingerprint) = state {
            state = try setup.confirmHost(fingerprint: fingerprint)
        }
        state = try setup.download(connection: connection, pin: pin, listener: Quiet())
        state = try setup.testDecrypt(connection: connection, pin: pin)
        guard case .done = state else {
            throw Failure(errorDescription: "Setup stopped at \(state).")
        }
        let sync = try? StoreSync.open(stateDir: stateDir.path)
        if let mode = UserDefaults.standard.string(forKey: "pawSimRecipients"), let sync {
            var lines = key.fingerprintHex() + "\n"
            if mode == "missing" {
                lines += SimulatedKey.newOther(pin: pin, touchRequired: false, touchTimeoutMs: 1_000).fingerprintHex() + "\n"
            }
            _ = try host.commitFile(
                path: "work/.gpg-id", contents: Data(lines.utf8), message: "Give work its own list of keys.")
            _ = try sync.refresh(connection: connection, pin: pin)
        }
        return Prepared(
            stateDir: stateDir.path, repoDir: stateDir.appendingPathComponent("repo").path,
            writer: StoreWriter.open(stateDir: stateDir.path), sync: sync, host: host)
    }

    /// Gives `model` its write side, or says on Home why it has none.
    @MainActor
    static func attach(_ prepared: Result<Prepared, Error>, to model: AppModel) {
        switch prepared {
        case .success(let prepared):
            model.edits = StoreEdits(
                writer: prepared.writer, sync: prepared.sync, repoDir: prepared.repoDir,
                sendingNote: "Sending comes when this phone is connected to a store.",
                keepAlive: prepared.host)
        case .failure(let error):
            model.notice = Notice(
                problem: "Adding passwords isn't available in this test store.", nextStep: describe(error))
        }
    }
}
#endif
