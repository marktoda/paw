import SwiftUI

/// The key sheet for the task a `KeyTaskDriver` follows.
struct KeyTaskSheet: View {
    @Bindable var driver: KeyTaskDriver
    /// What the key is for, e.g. "Open Fastmail".
    let purpose: String
    var note: String? = nil
    let onCancel: () -> Void

    var body: some View {
        KeySheetContent(
            purpose: purpose,
            stage: driver.stage,
            pin: $driver.pin,
            note: note,
            onContinue: { driver.continueWithPin() },
            onCancel: onCancel,
            onRetry: { driver.retry() })
        #if DEBUG && targetEnvironment(simulator)
            .modifier(SimulatedKeyControls(driver: driver))
        #endif
    }
}
