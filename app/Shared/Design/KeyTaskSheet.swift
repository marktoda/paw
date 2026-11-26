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
            comingSoon: comingSoon)
        #if DEBUG && targetEnvironment(simulator)
            .modifier(SimulatedKeyControls(driver: driver))
        #endif
    }
}
