import Foundation

#if DEBUG && targetEnvironment(simulator)
/// Setup in the simulator. `-pawSetup 1` starts with nothing set up (add
/// `-pawSetupKeep 1` to keep what earlier launches set up), with the core's
/// simulated git host running inside the app, serving the fixture store
/// to the simulated key. `-pawSimRemote` makes the host misbehave:
/// - `refuseKey`: it does not know the key until the debug control on
///   "doesn't know this key yet" adds it;
/// - `changedHostKey`: it presents another key once the first was trusted,
///   as an attacker in the way would;
/// - `dropMidClone`: the first download is cut off part way; the next works;
/// - `repositoryNotFound`: it accepts the key's login but answers every git
///   command as GitHub does for a repository the account can't see.
/// The simulated key's own arguments (`-pawSimPin` and the rest) apply too.
enum SetupLaunch {
    static var isRequested: Bool {
        LaunchArgument.setup.bool
    }

    static var keepsState: Bool {
        LaunchArgument.setupKeep.bool
    }

    fileprivate static var remoteMode: String? {
        LaunchArgument.simRemote.string
    }
}

/// The simulated git host, started once per launch when setup first needs it.
final class SimulatedHostHolder: @unchecked Sendable {
    static let shared = SimulatedHostHolder()

    /// Owned by `SimRemoteHandle`, so the sync screens' debug controls can
    /// replace it (a rewritten history is served on the same port).
    var host: SimRemote? {
        SimRemoteHandle.shared.host
    }

    let problem: String?

    private init() {
        do {
            let host = try SimRemote.start(key: SimulatedKeyConnector.launched.key)
            switch SetupLaunch.remoteMode {
            case "refuseKey": host.setRefuseKey(refuse: true)
            case "dropMidClone": host.setDropMidClone(drop: true)
            case "repositoryNotFound": host.setRepositoryNotFound(notFound: true)
            default: break
            }
            SimRemoteHandle.shared.register(host, key: SimulatedKeyConnector.launched.key)
            problem = nil
        } catch {
            problem = describe(error)
        }
    }

    /// "Add the key on the host": the host accepts the key from now on.
    func acceptKey() {
        host?.setRefuseKey(refuse: false)
    }

    /// The person trusted the host's key: with `changedHostKey`, the host
    /// presents another one from now on.
    func hostConfirmed() {
        if SetupLaunch.remoteMode == "changedHostKey" {
            host?.setChangedHostKey(changed: true)
        }
    }

    /// A download failed for a reason that was not the key's: with
    /// `dropMidClone`, the next one goes through.
    func downloadFailed() {
        if SetupLaunch.remoteMode == "dropMidClone" {
            host?.setDropMidClone(drop: false)
        }
    }
}
#endif
