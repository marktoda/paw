import SwiftUI
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

    /// What reading the store's files throws now (`Platform.simulated`).
    @MainActor static func refusal() -> Error? {
        if isLocked { return Unreadable() }
        return nil
    }

    @MainActor static func unlock() {
        isLocked = false
        NotificationCenter.default.post(name: UIApplication.protectedDataDidBecomeAvailableNotification, object: nil)
    }
}

/// For the UI tests: what is left of a store after it was removed, read
/// four times a second. "copies 0 · keychain resets 1 · pin none ·
/// pasteboard empty · legacy gone".
struct SimStoreState: View {
    let phase: StoreRootModel.Phase

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.25)) { _ in
            SimReadout(identifier: "sim-store-state", text: Self.state(phase), label: "Store state")
        }
    }

    @MainActor
    private static func state(_ phase: StoreRootModel.Phase) -> String {
        let copiesDir = (try? StoreLocation.standard())?.copiesDir
        let copies = copiesDir.flatMap { try? FileManager.default.contentsOfDirectory(atPath: $0.path).count } ?? 0
        let port: KeyPort? = switch phase {
        case .home(let model): model.storeSession.keyPort
        case .setup(let model): model.reader.port
        case .failed, .waitingForUnlock: nil
        }
        let pin = port?.session()?.holdsPin() == true ? "held" : "none"
        let pasteboard = UIPasteboard.general.hasStrings ? "full" : "empty"
        let legacy = LegacyStore.folderLeft ? "present" : "gone"
        return "copies \(copies) · keychain resets \(SimulatedKeystoreHolder.keystore.resetCount()) · pin \(pin)"
            + " · pasteboard \(pasteboard) · legacy \(legacy)"
    }
}
#endif
