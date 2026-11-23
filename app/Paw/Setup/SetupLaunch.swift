import Foundation

#if targetEnvironment(simulator)
/// Setup in the simulator. `-pawSetup 1` starts with nothing set up (add
/// `-pawSetupKeep 1` to keep what earlier launches set up), with the core's
/// simulated git host running inside the app, serving the fixture store
/// to the simulated key. `-pawSimRemote` makes the host misbehave:
/// - `refuseKey`: it does not know the key until the debug control on
///   "doesn't know this key yet" adds it;
/// - `changedHostKey`: it presents another key once the first was trusted,
///   as an attacker in the way would;
/// - `dropMidClone`: the first download is cut off part way; the next works.
/// The simulated key's own arguments (`-pawSimPin` and the rest) apply too.
enum SetupLaunch {
    static var isRequested: Bool {
        UserDefaults.standard.bool(forKey: "pawSetup")
    }

    static var keepsState: Bool {
        UserDefaults.standard.bool(forKey: "pawSetupKeep")
    }

    fileprivate static var remoteMode: String? {
        UserDefaults.standard.string(forKey: "pawSimRemote")
    }
}

/// The simulated git host, started once per launch when setup first needs it.
final class SimulatedHostHolder: @unchecked Sendable {
    static let shared = SimulatedHostHolder()

    let host: SimRemote?
    /// Why it did not start, if it did not.
    let problem: String?

    private init() {
        do {
            let host = try SimRemote.start(key: SimulatedKeyHolder.shared.key)
            switch SetupLaunch.remoteMode {
            case "refuseKey": host.setRefuseKey(refuse: true)
            case "dropMidClone": host.setDropMidClone(drop: true)
            default: break
            }
            self.host = host
            problem = nil
        } catch {
            host = nil
            problem = describe(error)
        }
    }

    /// Setup's hooks into the host and the key.
    func setupSimulator() -> SetupSimulator {
        let host = host
        let key = SimulatedKeyHolder.shared
        return SetupSimulator(
            address: host?.address() ?? "",
            hostConfirmed: {
                if SetupLaunch.remoteMode == "changedHostKey" {
                    host?.setChangedHostKey(changed: true)
                }
            },
            downloadFailed: {
                if SetupLaunch.remoteMode == "dropMidClone" {
                    host?.setDropMidClone(drop: false)
                }
            },
            acceptKey: { host?.setRefuseKey(refuse: false) },
            plugIn: { key.key.plugIn() },
            startsUnplugged: key.startsUnplugged)
    }
}
#endif
