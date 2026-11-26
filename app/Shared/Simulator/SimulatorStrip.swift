import SwiftUI

#if DEBUG && targetEnvironment(simulator)
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
