import SwiftUI

#if DEBUG && targetEnvironment(simulator)
/// A stand-in for the hardware, for the simulator's debug builds.
struct SimulatorAction {
    let title: String
    let identifier: String
    let perform: () -> Void
}

extension EnvironmentValues {
    /// The stand-ins a screen's simulator strip shows (the key sheet's, a
    /// setup page's), set from outside the screen by `simulatorActions(_:)`.
    @Entry var simulatorActions: [SimulatorAction] = []
    /// `false` keeps every stand-in off the screens (the gallery).
    @Entry var showsSimulatorControls = true
}

extension View {
    /// Gives the screen inside its simulator strip's controls.
    func simulatorActions(_ actions: [SimulatorAction]) -> some View {
        environment(\.simulatorActions, actions)
    }
}

/// The key sheet's stand-ins for the key its driver reads, by its stage.
struct SimulatedKeyControls: ViewModifier {
    let driver: KeyTaskDriver

    func body(content: Content) -> some View {
        content.simulatorActions(actions)
    }

    private var actions: [SimulatorAction] {
        guard let keys = driver.reader?.simulated else { return [] }
        switch driver.stage {
        case .touch:
            return [
                SimulatorAction(title: "touch the key", identifier: "simulated-touch") {
                    keys.key.touch()
                },
                SimulatorAction(title: "time out the touch", identifier: "simulated-touch-timeout") {
                    keys.key.timeOutTouch()
                },
            ]
        case .plugIn:
            return [SimulatorAction(title: "plug in the key", identifier: "simulated-plug-in") {
                keys.plugIn()
            }]
        case .failed where driver.readProblem != nil:
            return [SimulatorAction(title: "let the key answer again", identifier: "simulated-key-answers") {
                keys.stopAttachFailures()
            }]
        default:
            return []
        }
    }
}

/// The simulator's controls for one screen: a single small dashed strip
/// under everything else, marked with a hammer, so they take no room from
/// the design and a screenshot never shows them as part of it.
struct SimulatorStrip: View {
    let actions: [SimulatorAction]
    @Environment(\.showsSimulatorControls) private var shown

    var body: some View {
        if shown, !actions.isEmpty {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: PawMetrics.Spacing.xs) { controls }
                VStack(spacing: 0) { controls }
            }
            .padding(.horizontal, PawMetrics.Spacing.m)
            .overlay(
                RoundedRectangle(cornerRadius: PawMetrics.inlineRadius, style: .continuous).strokeBorder(
                    PawColor.labelTertiary, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
        }
    }

    @ViewBuilder private var controls: some View {
        Image(systemName: "hammer")
            .font(PawFont.caption)
            .foregroundStyle(PawColor.labelTertiary)
            .accessibilityHidden(true)
        ForEach(actions, id: \.identifier) { action in
            Button(action: action.perform) {
                Text(action.title)
                    .font(PawFont.caption.weight(.semibold))
                    .foregroundStyle(PawColor.labelSecondary)
                    .underline()
                    .lineLimit(1)
                    .padding(.horizontal, PawMetrics.Spacing.s)
                    .frame(minHeight: PawMetrics.tapMin)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(action.title)
            .accessibilityIdentifier(action.identifier)
        }
    }
}

/// One simulator control on its own, for a screen that has only one.
struct SimulatorActionButton: View {
    let action: SimulatorAction

    var body: some View {
        SimulatorStrip(actions: [action])
    }
}
#endif
