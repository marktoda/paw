import Foundation

#if targetEnvironment(simulator)
/// What another computer does to the simulated git host, for the sync
/// screens and their UI tests. `-pawSimRemoteChange <mode>` applies one at
/// launch, once the store is set up and before paw has fetched it, so the
/// next send or check finds it; the sync sheet's simulator section applies
/// any of them by hand.
/// - `add`: a new entry, `web/forum.example`;
/// - `edit`: `web/news.ycombinator.com` changes;
/// - `conflict`: `web/example.com` changes (edit it here too for "kept both");
/// - `delete`: not possible yet: the simulated host cannot remove a file;
/// - `gpgid`: `work/` gets its own `.gpg-id` (the same key);
/// - `link`: a name paw will not check out (`Web/github.com` beside
///   `web/`): the simulated host cannot commit a real link yet;
/// - `rewrite`: the history is replaced (force-pushed). At launch this only
///   prepares: the store is set up one commit past the fixture, so a fresh
///   fixture history no longer contains what paw saw. Apply it by hand.
/// `-pawSimRemoteRefusePushes 1` refuses every branch update, and
/// `-pawSimRemoteDrop 1` cuts the next connection off half way.
enum SimRemoteChange: String, CaseIterable, Identifiable {
    case add, edit, conflict, delete, gpgid, link, rewrite

    var id: String { rawValue }

    static var atLaunch: SimRemoteChange? {
        UserDefaults.standard.string(forKey: "pawSimRemoteChange").flatMap(SimRemoteChange.init(rawValue:))
    }

    /// The debug button's words.
    var title: String {
        switch self {
        case .add: "Add web/forum.example"
        case .edit: "Change web/news.ycombinator.com"
        case .conflict: "Change web/example.com"
        case .delete: "Delete an entry (not possible yet)"
        case .gpgid: "Give work/ its own .gpg-id"
        case .link: "Add a name paw won't check out"
        case .rewrite: "Replace the history"
        }
    }
}

/// The simulated git host the store syncs with, owned here alone so that
/// replacing its history can stop it and serve a new one on the same port.
final class SimRemoteHandle: @unchecked Sendable {
    static let shared = SimRemoteHandle()

    private let lock = NSLock()
    private var current: SimRemote?
    /// The key whose SSH key the host accepts.
    private var key: SimulatedKey?
    /// Where the store is checked out: the source of ciphertext for changes.
    private var repoDir: String?
    private var rewrites = 0
    private var launchChangeApplied = false
    private var refusesPushes = UserDefaults.standard.bool(forKey: "pawSimRemoteRefusePushes")
    private var dropsNext = UserDefaults.standard.bool(forKey: "pawSimRemoteDrop")

    var host: SimRemote? {
        lock.withLock { current }
    }

    /// Takes over `host`. With `-pawSimRemoteChange rewrite`, adds one
    /// commit before paw downloads the store (see `SimRemoteChange`).
    func register(_ host: SimRemote, key: SimulatedKey) {
        lock.withLock {
            current = host
            self.key = key
        }
        if SimRemoteChange.atLaunch == .rewrite {
            _ = try? host.commitFile(
                path: "README", contents: Data("A password store, before its history was replaced.\n".utf8),
                message: "Add a README.")
        }
    }

    /// The store is set up at `repoDir`: applies `-pawSimRemoteChange`
    /// (once per launch) and the push and connection flags.
    func storeReady(repoDir: String) {
        let change: SimRemoteChange? = lock.withLock {
            self.repoDir = repoDir
            guard !launchChangeApplied else { return nil }
            launchChangeApplied = true
            return SimRemoteChange.atLaunch
        }
        // Only now: setup's own download must not be refused or cut off.
        if let host {
            applyFlags(host)
        }
        if let change, change != .rewrite {
            _ = try? apply(change)
        }
    }

    /// Makes the change on the host as another computer would. Returns
    /// what happened, for the debug section. Blocks while a rewrite starts
    /// the new host: call off the main thread.
    func apply(_ change: SimRemoteChange) throws -> String {
        switch change {
        case .add:
            try commitCopy(of: "travel/airline", as: "web/forum.example", message: "Add given password for web/forum.example to store.")
            return "Added web/forum.example."
        case .edit:
            try commitCopy(of: "travel/rail.example", as: "web/news.ycombinator.com",
                           message: "Edit password for web/news.ycombinator.com using editor.")
            return "Changed web/news.ycombinator.com."
        case .conflict:
            try commitCopy(of: "travel/airline", as: "web/example.com",
                           message: "Edit password for web/example.com using editor.")
            return "Changed web/example.com."
        case .delete:
            return "The simulated host can't delete a file yet."
        case .gpgid:
            guard let host, let key = lock.withLock({ self.key }) else { return "No simulated host." }
            _ = try host.commitFile(
                path: "work/.gpg-id", contents: Data((key.fingerprintHex() + "\n").utf8),
                message: "Give work its own list of keys.")
            return "work/ has its own .gpg-id."
        case .link:
            try commitCopy(of: "travel/airline", as: "Web/github.com", message: "Add given password for Web/github.com to store.")
            return "Added Web/github.com beside web/."
        case .rewrite:
            try rewrite()
            return "Replaced the history."
        }
    }

    func setRefusesPushes(_ refuse: Bool) {
        lock.withLock { refusesPushes = refuse }
        host?.setRefusePushes(refuse: refuse)
    }

    var refusingPushes: Bool {
        lock.withLock { refusesPushes }
    }

    /// The next connection is cut off half way through what it fetches.
    func dropNextConnection() {
        lock.withLock { dropsNext = true }
        host?.setDropMidClone(drop: true)
    }

    var droppingNext: Bool {
        lock.withLock { dropsNext }
    }

    /// A sync failed: a dropped connection drops only once.
    func syncFailed() {
        let wasDropping: Bool = lock.withLock {
            let was = dropsNext
            dropsNext = false
            return was
        }
        if wasDropping {
            host?.setDropMidClone(drop: false)
        }
    }

    func setChangedHostKey(_ changed: Bool) {
        host?.setChangedHostKey(changed: changed)
    }

    /// "tip 1a2b3c4 · 3 logins", for the debug element UI tests read.
    var summary: String {
        guard let host else { return "no host" }
        let tip = host.tip().map { String($0.prefix(12)) } ?? "none"
        return "tip \(tip) · \(host.logins()) logins"
    }

    private func applyFlags(_ host: SimRemote) {
        let (refuse, drop) = lock.withLock { (refusesPushes, dropsNext) }
        host.setRefusePushes(refuse: refuse)
        host.setDropMidClone(drop: drop)
    }

    /// Commits the checked-out ciphertext of `source` as `entry`: it opens
    /// with the simulated key, showing `source`'s text.
    private func commitCopy(of source: String, as entry: String, message: String) throws {
        guard let host, let repoDir = lock.withLock({ self.repoDir }) else {
            throw SimRemoteProblem(errorDescription: "The store isn't set up yet.")
        }
        let contents = try Data(contentsOf: URL(fileURLWithPath: repoDir).appendingPathComponent(source + ".gpg"))
        _ = try host.commitFile(path: entry + ".gpg", contents: contents, message: message)
    }

    /// Stops the host and serves a fresh fixture history on the same port,
    /// with one more commit that differs on each rewrite, so it never
    /// contains the tip paw saw last.
    private func rewrite() throws {
        // The old host stops when its last reference goes, in here.
        let taken: (port: UInt16, key: SimulatedKey)? = lock.withLock {
            guard let old = current, let key else { return nil }
            current = nil
            return (old.port(), key)
        }
        guard let taken else {
            throw SimRemoteProblem(errorDescription: "No simulated host.")
        }
        let port = taken.port
        let key = taken.key
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("paw-sim-rewrite-\(UUID().uuidString)", isDirectory: true)
        let fresh = try SimRemote.serve(key: key, dir: dir.path, port: port)
        let count = lock.withLock { () -> Int in
            rewrites += 1
            current = fresh
            return rewrites
        }
        applyFlags(fresh)
        try commitCopy(
            of: "travel/airline", as: "web/new-history-\(count)",
            message: "Add given password for web/new-history-\(count) to store.")
    }
}

struct SimRemoteProblem: LocalizedError {
    let errorDescription: String?
}
#endif
