import Foundation

/// Which build this is, for Settings: the commit it was built from and the
/// Xcode configuration (Debug or Release), from the Info.plist keys
/// `app/project.yml` sets. CI passes the commit (`PAW_COMMIT`); a build
/// made without it (`unknown`, the project's default) leaves it out.
enum BuildStamp {
    static var commit: String? { value("PawCommit") }
    static var configuration: String? { value("PawConfiguration") }

    /// "1a2b3c4 · Release", or what of it is known; `nil` when neither is.
    static var words: String? {
        let known = [commit, configuration].compactMap { $0 }
        return known.isEmpty ? nil : known.joined(separator: " \u{00B7} ")
    }

    private static func value(_ key: String) -> String? {
        guard let text = Bundle.main.object(forInfoDictionaryKey: key) as? String,
              !text.isEmpty, !text.hasPrefix("$("), text != "unknown" else { return nil }
        return text
    }
}
