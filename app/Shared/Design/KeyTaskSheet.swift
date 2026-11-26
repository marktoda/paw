import SwiftUI

/// The key sheet for the task a `KeyTaskDriver` follows: its stage, the PIN
/// field bound to it, Continue and Try again, and in the simulator the
/// controls that stand in for the key.
struct KeyTaskSheet: View {
    @Bindable var driver: KeyTaskDriver
    /// What the key is for, e.g. "Open Fastmail".
    let purpose: String
    var note: String? = nil
    /// A way on that is not built yet, on the failed stage.
    var comingSoon: String? = nil
    let onCancel: () -> Void

    var body: some View {
        KeySheetContent(
            purpose: purpose,
            stage: driver.stage,
            pin: $driver.pin,
            note: note,
            onContinue: { driver.continueWithPin() },
            onCancel: onCancel,
            onRetry: { driver.retry() },
            simulatorActions: simulatorActions,
            comingSoon: comingSoon)
    }

    /// In the simulator: a finger on the key, the key's touch timeout
    /// running out now, a key going in, and a key that answers again after
    /// its reads failed.
    private var simulatorActions: [SimulatorAction] {
        #if DEBUG && targetEnvironment(simulator)
        switch driver.stage {
        case .touch:
            return [
                SimulatorAction(title: "touch the key", identifier: "simulated-touch") {
                    SimulatedKeyHolder.shared.key.touch()
                },
                SimulatorAction(title: "time out the touch", identifier: "simulated-touch-timeout") {
                    SimulatedKeyHolder.shared.key.timeOutTouch()
                },
            ]
        case .plugIn:
            let reader = driver.reader
            return [SimulatorAction(title: "plug in the key", identifier: "simulated-plug-in") {
                reader?.simulatedPlugIn()
            }]
        case .failed where driver.readProblem != nil:
            return [SimulatorAction(title: "let the key answer again", identifier: "simulated-key-answers") {
                SimulatedKeyHolder.shared.stopAttachFailures()
            }]
        default:
            return []
        }
        #else
        return []
        #endif
    }
}
