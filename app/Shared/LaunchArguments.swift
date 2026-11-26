import Foundation

#if DEBUG
/// A debug launch argument (`-pawFixture 1`, `-pawSimPin 123456`), read
/// from this launch's command line only. `UserDefaults` would also answer
/// from values persisted in the app's defaults, which survive launches and
/// could switch a hook on with nobody passing the argument.
///
/// Debug builds only: a Release build reads no launch argument at all, so
/// nothing passed on its command line can switch a harness on.
///
/// The arguments are declared in two lists, by who reads them:
/// `KeyLaunchArgument` here, read in both processes, and the app's own
/// `LaunchArgument` (app/Paw/AppLaunchArguments.swift).
/// `scripts/launch-arguments.sh` reads both: every argument a UI test
/// passes must be declared, every one declared must be read somewhere, and
/// `scripts/check-no-sim.sh` looks for every name in the device build.
protocol DebugLaunchArgument: RawRepresentable where RawValue == String {}

extension DebugLaunchArgument {
    /// The word after `-name`, if this launch was given it.
    var string: String? {
        let arguments = ProcessInfo.processInfo.arguments
        guard let index = arguments.firstIndex(of: "-" + rawValue), arguments.indices.contains(index + 1) else {
            return nil
        }
        return arguments[index + 1]
    }

    /// `-name 1` (or YES, true).
    var bool: Bool {
        guard let value = string?.lowercased() else { return false }
        return ["1", "yes", "true"].contains(value)
    }

    var int: Int? {
        string.flatMap { Int($0) }
    }
}

/// The simulated key's launch arguments (`SimulatedKeyConnector.launched`),
/// read in the simulator by the app and the AutoFill extension alike.
enum KeyLaunchArgument: String, CaseIterable, DebugLaunchArgument {
    /// Word: its PIN (default 123456).
    case simPin = "pawSimPin"
    /// Flag: it needs a touch (default on).
    case simTouch = "pawSimTouch"
    /// Number: how long it waits for the touch, in milliseconds.
    case simTouchTimeoutMs = "pawSimTouchTimeoutMs"
    /// Number: PIN tries left.
    case simPinTries = "pawSimPinTries"
    /// Word, `unsupported`: a PIN KDF paw can't follow.
    case simKdf = "pawSimKdf"
    /// Number: how many of its first reads fail.
    case simAttachFails = "pawSimAttachFails"
    /// Flag: it is out at launch.
    case simUnplugged = "pawSimUnplugged"
}
#endif
