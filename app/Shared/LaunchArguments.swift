import Foundation

/// The debug launch arguments (`-pawFixture 1`, `-pawSimPin 123456`), read
/// from this launch's command line only. `UserDefaults` would also answer
/// from values persisted in the app's defaults, which survive launches and
/// could switch a hook on with nobody passing the argument.
enum LaunchArguments {
    /// The word after `-name`, if this launch was given it.
    static func string(_ name: String) -> String? {
        let arguments = ProcessInfo.processInfo.arguments
        guard let index = arguments.firstIndex(of: "-" + name), arguments.indices.contains(index + 1) else {
            return nil
        }
        return arguments[index + 1]
    }

    /// `-name 1` (or YES, true).
    static func bool(_ name: String) -> Bool {
        guard let value = string(name)?.lowercased() else { return false }
        return ["1", "yes", "true"].contains(value)
    }

    static func int(_ name: String) -> Int? {
        string(name).flatMap { Int($0) }
    }
}
