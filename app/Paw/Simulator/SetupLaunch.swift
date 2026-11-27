import Foundation

#if DEBUG && targetEnvironment(simulator)
/// Setup in the simulator. `-pawSetup 1` starts with nothing set up (add
/// `-pawSetupKeep 1` to keep what earlier launches set up), with the core's
/// simulated git host running inside the app, serving the fixture store
/// to the simulated key (`SimRemoteHandle.setupHost`). `-pawSimRemote`
/// makes the host misbehave (`SimRemoteMode`). The simulated key's own
/// arguments (`-pawSimPin` and the rest) apply too.
enum SetupLaunch {
    static var isRequested: Bool {
        LaunchArgument.setup.bool
    }

    static var keepsState: Bool {
        LaunchArgument.setupKeep.bool
    }
}

/// `-pawSimRemote <mode>`: how setup's simulated git host misbehaves.
enum SimRemoteMode: String {
    /// It does not know the key until the debug control on "doesn't know
    /// this key yet" adds it.
    case refuseKey
    /// It presents another key once the first was trusted, as an attacker
    /// in the way would.
    case changedHostKey
    /// The first download is cut off part way; the next works.
    case dropMidClone
    /// It accepts the key's login but answers every git command as GitHub
    /// does for a repository the account can't see.
    case repositoryNotFound

    static var atLaunch: SimRemoteMode? {
        LaunchArgument.simRemote.string.flatMap(SimRemoteMode.init(rawValue:))
    }
}
#endif
