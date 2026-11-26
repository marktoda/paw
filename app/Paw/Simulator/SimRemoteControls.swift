import SwiftUI

#if DEBUG && targetEnvironment(simulator)
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
/// - `secondKey`: `work/` gets its own `.gpg-id` naming the key and a
///   second simulated key whose public half paw is given first, so a
///   change saved in `work` before it is held back and can be re-encrypted;
/// - `link`: a name paw will not check out (`Web/github.com` beside
///   `web/`): the simulated host cannot commit a real link yet;
/// - `rewrite`: the history is replaced (force-pushed). At launch this only
///   prepares: the store is set up one commit past the fixture, so a fresh
///   fixture history no longer contains what paw saw. Apply it by hand;
/// - `hostKey`: the host presents another host key from now on.
/// `-pawSimRemoteRefusePushes 1` refuses every branch update, and
/// `-pawSimRemoteDrop 1` cuts the next connection off half way.
enum SimRemoteChange: String, CaseIterable, Identifiable {
    case add, edit, conflict, delete, gpgid, link, rewrite, secondKey, hostKey

    var id: String { rawValue }

    static var atLaunch: SimRemoteChange? {
        LaunchArgument.simRemoteChange.string.flatMap(SimRemoteChange.init(rawValue:))
    }

    var title: String {
        switch self {
        case .add: "Add web/forum.example"
        case .edit: "Change web/news.ycombinator.com"
        case .conflict: "Change web/example.com"
        case .delete: "Delete an entry (not possible yet)"
        case .gpgid: "Give work/ its own .gpg-id"
        case .link: "Add a name paw won't check out"
        case .rewrite: "Replace the history"
        case .secondKey: "Give work/ a second key"
        case .hostKey: "Present another host key"
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
    private var refusesPushes = LaunchArgument.simRemoteRefusePushes.bool
    private var dropsNext = LaunchArgument.simRemoteDrop.bool

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
        case .secondKey:
            let (key, repoDir) = lock.withLock { (self.key, self.repoDir) }
            guard let host, let key, let repoDir else { return "No simulated host." }
            // Only its public half is read, so the PIN does not matter.
            let other = SimulatedKey.newOther(pin: "", touchRequired: false, touchTimeoutMs: 1_000)
            let stateDir = URL(fileURLWithPath: repoDir).deletingLastPathComponent().path
            _ = try other.pinAsCard(stateDir: stateDir)
            let lines = key.fingerprintHex() + "\n" + other.fingerprintHex() + "\n"
            _ = try host.commitFile(
                path: "work/.gpg-id", contents: Data(lines.utf8), message: "Give work a second key.")
            return "work/ has a second key."
        case .hostKey:
            guard let host else { return "No simulated host." }
            host.setChangedHostKey(changed: true)
            return "Presenting another host key."
        }
    }

    func setRefusesPushes(_ refuse: Bool) {
        lock.withLock { refusesPushes = refuse }
        host?.setRefusePushes(refuse: refuse)
    }

    var refusingPushes: Bool {
        lock.withLock { refusesPushes }
    }

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
            guard let old = self.current, let key = self.key else { return nil }
            self.current = nil
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
/// The simulated host's tip and logins, and what its debug controls last
/// did, for the sync screens' debug section and the UI tests.
@MainActor @Observable
final class SimRemotePanel {
    static let shared = SimRemotePanel()

    private(set) var summary = ""
    private(set) var status = ""
    @ObservationIgnored private let queue = DispatchQueue(label: "paw-sim-remote", qos: .userInitiated)

    /// A sync model opened on the store at `repoDir`.
    func storeReady(repoDir: String) {
        SimRemoteHandle.shared.storeReady(repoDir: repoDir)
        status = ""
        refresh()
    }

    func syncFailed() {
        SimRemoteHandle.shared.syncFailed()
        refresh()
    }

    func refresh() {
        summary = SimRemoteHandle.shared.summary
    }

    /// Runs off the main thread: a rewrite starts a new host.
    func apply(_ change: SimRemoteChange) {
        status = "Working\u{2026}"
        queue.async { [weak self] in
            let said: String
            do {
                said = try SimRemoteHandle.shared.apply(change)
            } catch {
                said = "Failed: \(describe(error))"
            }
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.status = said
                    self?.refresh()
                }
            }
        }
    }

    func setRefusesPushes(_ refuse: Bool) {
        SimRemoteHandle.shared.setRefusesPushes(refuse)
        status = refuse ? "Refusing pushes." : "Accepting pushes."
    }

    func dropNextConnection() {
        SimRemoteHandle.shared.dropNextConnection()
        status = "The next connection drops half way."
    }
}

/// The simulated host follows sync as its screens show it: the launch's
/// change once the store is on screen, and after each sync, its tip for
/// the UI tests; a sync that failed stops a dropped connection from
/// dropping again.
struct SimRemoteWatch: ViewModifier {
    let model: AppModel

    @ViewBuilder func body(content: Content) -> some View {
        if let syncing = model.storeSession.syncing {
            content
                .background(alignment: .topLeading) {
                    SimRemoteSummary()
                }
                .onAppear {
                    SimRemotePanel.shared.storeReady(repoDir: model.storeSession.live.repoDir)
                }
                .onChange(of: syncing.running == nil) { _, idle in
                    guard idle else { return }
                    if Self.failed(syncing) {
                        SimRemotePanel.shared.syncFailed()
                    } else {
                        SimRemotePanel.shared.refresh()
                    }
                }
        } else {
            content
        }
    }

    private static func failed(_ syncing: SyncModel) -> Bool {
        if let failure = syncing.failure {
            return failure.kind != .cancelled
        }
        guard case .key? = syncing.sheet?.mode, case .failed = syncing.keyTask.stage else { return false }
        return true
    }
}

/// The simulated host's tip, as an element UI tests read.
struct SimRemoteSummary: View {
    var body: some View {
        Text(SimRemotePanel.shared.summary)
            .font(.caption2)
            .frame(width: 1, height: 1)
            .opacity(0.02)
            .accessibilityIdentifier("sim-remote-summary")
    }
}

/// Debug controls for the simulated git host, drawn so a screenshot never
/// shows them as part of the design.
struct SimRemoteSection: View {
    @State private var shown = false
    private var panel: SimRemotePanel { .shared }

    var body: some View {
        Section {
            SimulatorDisclosureRow(shown: $shown)
            if shown {
                controls
            }
        }
    }

    @ViewBuilder private var controls: some View {
            // First, so it is on screen as soon as the controls open.
            Text(panel.status.isEmpty ? panel.summary : "\(panel.status) \(panel.summary)")
                .font(PawFont.footnote.monospaced())
                .foregroundStyle(PawColor.labelSecondary)
                .accessibilityIdentifier("sim-remote-status")
            ForEach(SimRemoteChange.allCases) { change in
                Button(change.title) { panel.apply(change) }
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("sim-remote-\(change.rawValue)")
            }
            Button(SimRemoteHandle.shared.refusingPushes ? "Accept pushes again" : "Refuse pushes") {
                panel.setRefusesPushes(!SimRemoteHandle.shared.refusingPushes)
            }
            .foregroundStyle(PawColor.label)
            .accessibilityIdentifier("sim-remote-refuse-pushes")
            Button("Drop the next connection") { panel.dropNextConnection() }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-remote-drop")
    }
}
#endif
