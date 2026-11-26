import UIKit

#if DEBUG && targetEnvironment(simulator)
/// `-pawSimLockedLaunch 1`: launch as iOS does in the background while the
/// phone is locked, with the store's files unreadable, until the debug
/// "unlock" control posts the system's own unlock notification.
enum SimLockedLaunch {
    struct Unreadable: Error, CustomStringConvertible {
        var description: String { "Operation not permitted (os error 1) (simulated lock)" }
    }

    @MainActor static var isLocked = LaunchArgument.simLockedLaunch.bool

    @MainActor static func unlock() {
        isLocked = false
        NotificationCenter.default.post(name: UIApplication.protectedDataDidBecomeAvailableNotification, object: nil)
    }
}

extension StoreRootModel {
    /// "copies 0 · keychain resets 1 · pin none · pasteboard empty · legacy gone".
    func debugState() -> String {
        let copies = location.flatMap { try? FileManager.default.contentsOfDirectory(atPath: $0.copiesDir.path).count } ?? 0
        let pin = keyPort?.session()?.holdsPin() == true ? "held" : "none"
        let pasteboard = UIPasteboard.general.hasStrings ? "full" : "empty"
        return "copies \(copies) · keychain resets \(keystore.resetCount()) · pin \(pin) · pasteboard \(pasteboard)"
            + " · legacy \(legacyFolder)"
    }
}
#endif
