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
}

extension View {
    /// Gives the screen inside its simulator strip's controls.
    func simulatorActions(_ actions: [SimulatorAction]) -> some View {
        environment(\.simulatorActions, actions)
    }
}

/// The key sheet's stand-ins for the key, by its stage: a finger on the
/// key, the key's touch timeout running out now, a key going in, and a key
/// that answers again after its reads failed.
struct SimulatedKeyControls: ViewModifier {
    let driver: KeyTaskDriver

    func body(content: Content) -> some View {
        content.simulatorActions(actions)
    }

    private var actions: [SimulatorAction] {
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
            return [SimulatorAction(title: "plug in the key", identifier: "simulated-plug-in") {
                SimulatedKeyHolder.shared.plugIn()
            }]
        case .failed where driver.readProblem != nil:
            return [SimulatorAction(title: "let the key answer again", identifier: "simulated-key-answers") {
                SimulatedKeyHolder.shared.stopAttachFailures()
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

    var body: some View {
        if !actions.isEmpty {
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
