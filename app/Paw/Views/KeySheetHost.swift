import SwiftUI

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
            note: "Afterwards it opens with Face ID for \(AppModel.faceIdWindowSecs / 3600) hours.",
            onContinue: { model.continueWithPin() },
            onCancel: { model.cancelKeyTask() },
            onRetry: { model.retry() },
            simulatorAction: simulatorAction)
            .presentationDetents([.height(PawMetrics.keySheetHeight), .large], selection: $detent)
            .presentationDragIndicator(.visible)
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
                    Text("Ends every Face ID copy and forgets your PIN. Every password then needs your key.")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
                #if targetEnvironment(simulator)
                if model.simulator != nil {
                    Section {
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
                    } header: {
                        PawSectionHeader("Simulator")
                    }
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
        .presentationDetents([.medium, .large])
        .privacyCover()
    }
}
