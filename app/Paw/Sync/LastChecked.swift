import Foundation

/// When paw last found the store as the server has it, by a check with
/// the key or the sign-in, in the foreground or the background. Kept per
/// store in the app's own defaults, which can be read while the iPhone is
/// locked.
enum LastChecked {
    static func read(repoDir: String) -> Date? {
        let seen = UserDefaults.standard.double(forKey: key(repoDir))
        return seen > 0 ? Date(timeIntervalSince1970: seen) : nil
    }

    static func record(_ date: Date, repoDir: String) {
        UserDefaults.standard.set(date.timeIntervalSince1970, forKey: key(repoDir))
    }

    /// The store is removed from this iPhone.
    static func forget(repoDir: String) {
        UserDefaults.standard.removeObject(forKey: key(repoDir))
    }

    private static func key(_ repoDir: String) -> String {
        "pawSyncLastChecked:\(repoDir)"
    }
}
