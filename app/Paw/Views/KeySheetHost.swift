import SwiftUI
import UIKit

/// The one key sheet, following `model.keyRequest`. Tall while the keyboard
/// is up for the PIN, 480pt otherwise.
struct KeySheetHost: View {
    @Bindable var model: AppModel
    @State private var detent = PresentationDetent.height(PawMetrics.keySheetHeight)

    private var stage: KeySheetStage {
        model.keyRequest?.stage ?? .busy
    }

    var body: some View {
        KeySheetContent(
            purpose: "Open \(model.keyRequest?.entry.name ?? "")",
            stage: stage,
            pin: $model.pinEntry,
            note: model.copies == nil
                ? nil : "Then it opens with Face ID for \(AppModel.faceIdWindowSecs / 3600) hours.",
            onContinue: { model.continueWithPin() },
            onCancel: { model.cancelKeyTask() },
            onRetry: { model.retry() },
            simulatorAction: simulatorAction,
            moreSimulatorActions: moreSimulatorActions,
            comingSoon: model.keyRequest?.differentKey == true ? "Replace key (coming)" : nil)
            .presentationDetents([.height(PawMetrics.keySheetHeight), .large], selection: $detent)
            .presentationDragIndicator(.visible)
            .pawSheetBackground()
            .onChange(of: stage, initial: true) {
                detent = Self.needsKeyboard(stage) ? .large : .height(PawMetrics.keySheetHeight)
            }
            .privacyCover()
    }

    /// In the simulator, a finger on the key and a key going in.
    private var simulatorAction: SimulatorAction? {
        #if targetEnvironment(simulator)
        switch stage {
        case .touch:
            return SimulatorAction(title: "touch the key", identifier: "simulated-touch") {
                KeyConnection.simulateTouch()
            }
        case .plugIn:
            return SimulatorAction(title: "plug in the key", identifier: "simulated-plug-in") {
                model.simulatedPlugIn()
            }
        default:
            return nil
        }
        #else
        return nil
        #endif
    }

    /// In the simulator, the key's touch timeout running out now.
    private var moreSimulatorActions: [SimulatorAction] {
        #if targetEnvironment(simulator)
        if case .failed = stage, model.attachProblem != nil {
            return [SimulatorAction(title: "let the key answer again", identifier: "simulated-key-answers") {
                KeyConnection.simulateKeyAnswersAgain()
            }]
        }
        guard stage == .touch else { return [] }
        return [SimulatorAction(title: "time out the touch", identifier: "simulated-touch-timeout") {
            KeyConnection.simulateTouchTimeout()
        }]
        #else
        return []
        #endif
    }

    private static func needsKeyboard(_ stage: KeySheetStage) -> Bool {
        switch stage {
        case .pin, .wrongPin, .pinAgain: true
        default: false
        }
    }
}

/// Settings, for now: Lock now, and in the simulator the controls for the
/// simulated hardware.
struct SettingsSheet: View {
    let model: AppModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Button {
                        model.lockNow()
                        dismiss()
                    } label: {
                        Text("Lock now")
                            .font(PawFont.body.weight(.semibold))
                            .foregroundStyle(PawColor.danger)
                    }
                    .accessibilityIdentifier("settings-lock-now")
                } footer: {
                    Text("Every password needs your key and its PIN again.")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
                AutoFillSettingsSection()
                StoreSettingsSections(model: model)
                LicencesSettingsSection()
                #if DEBUG && targetEnvironment(simulator)
                if model.simulator != nil {
                    SimulatorSettingsSection(model: model)
                }
                #endif
            }
            .listStyle(.insetGrouped)
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                        .accessibilityIdentifier("settings-done")
                }
            }
        }
        // Tall from the start: Remove is the last row and must not open
        // below the fold.
        .presentationDetents([.large])
        .privacyCover()
    }
}

#if DEBUG && targetEnvironment(simulator)
/// Settings' controls for the simulated hardware, and readbacks for the UI
/// tests. Debug simulator builds only: one closed row at the bottom of
/// Settings until it is tapped.
private struct SimulatorSettingsSection: View {
    let model: AppModel
    @Environment(\.storeControls) private var controls
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
            Button("Move the clock 1 hour on") { model.advanceSimulatedClock(hours: 1) }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-advance-1h")
            Button("Move the clock 9 hours on") { model.advanceSimulatedClock(hours: 9) }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-advance-9h")
            // A touch that comes after the sheet was closed.
            Button("Touch the simulated key") { KeyConnection.simulateTouch() }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-touch-key")
            Button("Let the key answer when read") { KeyConnection.simulateKeyAnswersAgain() }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-key-answers")
            Button("Let the next open finish after Cancel") { model.simulatesLateCancelNext = true }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("sim-late-cancel-next")
            // Ends a touch wait the sheet let go of, as the key's timeout would.
            Button("Time out the key's touch") { KeyConnection.simulateTouchTimeout() }
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
            if let protection = controls?.fileProtection() {
                Text(protection)
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityValue(protection)
                    .accessibilityIdentifier("sim-file-protection")
            }
    }
}
#endif

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
