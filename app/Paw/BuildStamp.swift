import Foundation

/// Which build this is, for Settings: the commit it was built from and the
/// Xcode configuration (Debug or Release), from the Info.plist keys
/// `app/project.yml` sets. CI passes the commit (`PAW_COMMIT`); a build
/// made without it says "unknown".
enum BuildStamp {
    static var commit: String { value("PawCommit") ?? "unknown" }
    static var configuration: String { value("PawConfiguration") ?? "unknown" }

    /// "1a2b3c4 · Release".
    static var words: String { "\(commit) \u{00B7} \(configuration)" }

    private static func value(_ key: String) -> String? {
        guard let text = Bundle.main.object(forInfoDictionaryKey: key) as? String,
              !text.isEmpty, !text.hasPrefix("$(") else { return nil }
        return text
    }
}
