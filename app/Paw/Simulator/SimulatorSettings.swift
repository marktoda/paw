import SwiftUI
import UIKit

#if DEBUG && targetEnvironment(simulator)
/// Settings' controls for the simulated hardware, and readbacks for the UI
/// tests. Debug simulator builds only: one closed row at the bottom of
/// Settings until it is tapped.
struct SimulatorSettingsSection: View {
    let model: AppModel
    @State private var pasteboard: String?
    @State private var shown = false

    var body: some View {
        Section {
            SimulatorDisclosureRow(shown: $shown)
            if shown {
                controlRows
            }
        }
    }

    @ViewBuilder private var controlRows: some View {
            Button("Move the clock 1 hour on") { advanceClock(hours: 1) }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-advance-1h")
            Button("Move the clock 9 hours on") { advanceClock(hours: 9) }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-advance-9h")
            // A touch that comes after the sheet was closed.
            Button("Touch the simulated key") { SimulatedKeyHolder.shared.key.touch() }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-touch-key")
            Button("Let the key answer when read") { SimulatedKeyHolder.shared.stopAttachFailures() }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-key-answers")
            Button("Let the next open finish after Cancel") { SimulatedLateCancel.armed = true }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-late-cancel-next")
            // Ends a touch wait the sheet let go of, as the key's timeout would.
            Button("Time out the key's touch") { SimulatedKeyHolder.shared.key.timeOutTouch() }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-time-out-touch")
            // What the system says just before the iPhone locks.
            Button("Lock the iPhone") {
                NotificationCenter.default.post(
                    name: UIApplication.protectedDataWillBecomeUnavailableNotification, object: nil)
            }
            .foregroundStyle(PawColor.label)
            .accessibilityIdentifier("sim-device-lock")
            // Read inside paw, so the system asks no paste permission.
            Button("Read the pasteboard") { pasteboard = UIPasteboard.general.string ?? "" }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-read-pasteboard")
            if let pasteboard {
                Text(pasteboard.isEmpty ? "Empty" : "Holds text")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityValue(pasteboard)
                    .accessibilityIdentifier("sim-pasteboard-value")
            }
            if let protection = model.storeSession.setUp?.location.protectionSummary() {
                Text(protection)
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityValue(protection)
                    .accessibilityIdentifier("sim-file-protection")
            }
    }

    private func advanceClock(hours: Int64) {
        SimulatedClockHolder.shared.advance(hours: hours)
        model.timePassed()
    }
}

/// "Simulator", with a hammer: opens and closes a list's debug controls.
/// Closed, it is the only debug row a screenshot shows.
struct SimulatorDisclosureRow: View {
    @Binding var shown: Bool

    var body: some View {
        Button {
            shown.toggle()
        } label: {
            HStack(spacing: PawMetrics.Spacing.s) {
                Image(systemName: "hammer")
                    .font(PawFont.caption)
                Text("Simulator")
                    .font(PawFont.footnote.weight(.semibold))
                Spacer()
                Image(systemName: PawSymbol.disclosure)
                    .font(PawFont.caption.weight(.semibold))
                    .rotationEffect(.degrees(shown ? 90 : 0))
            }
            .foregroundStyle(PawColor.labelSecondary)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Simulator controls")
        .accessibilityValue(shown ? "shown" : "hidden")
        .accessibilityIdentifier("sim-controls")
    }
}
#endif
