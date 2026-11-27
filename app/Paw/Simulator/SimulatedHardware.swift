import Foundation
import UIKit

#if DEBUG && targetEnvironment(simulator)
/// One simulated clock per launch, for the copies, the key port and the
/// clipboard: the debug controls move it on, and it moves on by
/// `-pawSimAwayMs` each time paw comes back from the background (counted
/// before paw sees it has come back, so the time away is as long as that).
@MainActor
final class SimulatedClockHolder {
    static let shared = SimulatedClockHolder()
    let clock: SimClock
    private var awayWatch: NSObjectProtocol?

    private init() {
        let clock = SimClock(wallMs: Int64(Date().timeIntervalSince1970 * 1000))
        self.clock = clock
        let away = UInt64(max(LaunchArgument.simAwayMs.int ?? 0, 0))
        guard away > 0 else { return }
        awayWatch = NotificationCenter.default.addObserver(
            forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main
        ) { _ in
            clock.advanceMs(ms: away)
        }
    }

    func advance(hours: Int64) {
        clock.advanceMs(ms: UInt64(hours) * 3_600_000)
    }
}
/// The app's platform on the launch's simulated hardware: its key
/// (`-pawSimPin`, `-pawSimTouch`, `-pawSimUnplugged`, …), clock and Face ID
/// (`SimulatedClockHolder`, `SimulatedKeystoreHolder`), its lock
/// (`-pawSimLockedLaunch`), and how long its pasteboard keeps a copy
/// (`-pawSimPasteboardSecs`).
enum LaunchedHardware {
    @MainActor static var app: Platform {
        .simulated(
            .app, keys: .launched, clock: SimulatedClockHolder.shared.clock.asClock(),
            keystore: SimulatedKeystoreHolder.keystore.asKeystore(), lockedOut: SimLockedLaunch.refusal,
            pasteboardLifetime: LaunchArgument.simPasteboardSecs.int.map { TimeInterval($0) })
    }

    /// `app` for `StoreRoot`, which reads the store only after this.
    @MainActor static var storeRoot: Platform {
        _ = launchedStore
        return app
    }

    /// Once per launch: `-pawSetup 1` deletes what earlier launches set up
    /// (unless `-pawSetupKeep 1`), and `-pawPlantLegacy 1` leaves a store
    /// where paw kept it before, for the next launch to delete.
    private static let launchedStore: Void = {
        if SetupLaunch.isRequested, !SetupLaunch.keepsState {
            try? StoreLocation.standard().removeAll()
        }
        if LegacyStore.plantsInstead {
            LegacyStore.plant()
        }
    }()
}
#endif
