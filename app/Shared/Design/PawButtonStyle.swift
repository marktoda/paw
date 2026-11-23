import SwiftUI

/// The two capsule buttons. A screen has at most one primary button.
struct PawButtonStyle: ButtonStyle {
    enum Kind {
        /// `onAction` on `action`: the one primary button on a screen.
        case primary
        /// `label` on `fill`.
        case secondary
        /// `danger` on `dangerFill`: the step that loses something. It
        /// asks before it acts.
        case destructive
    }

    let kind: Kind

    func makeBody(configuration: Configuration) -> some View {
        PawButtonBody(kind: kind, configuration: configuration)
    }
}

private struct PawButtonBody: View {
    let kind: PawButtonStyle.Kind
    let configuration: ButtonStyleConfiguration
    @Environment(\.isEnabled) private var isEnabled

    var body: some View {
        configuration.label
            .font(PawFont.headline)
            .multilineTextAlignment(.center)
            .foregroundStyle(foreground)
            .padding(.horizontal, PawMetrics.Spacing.l)
            .padding(.vertical, PawMetrics.Spacing.s)
            .frame(maxWidth: .infinity, minHeight: PawMetrics.control)
            .background(ground, in: Capsule())
            .contentShape(Capsule())
            .opacity(isEnabled ? (configuration.isPressed ? 0.7 : 1) : 0.4)
    }

    private var foreground: Color {
        switch kind {
        case .primary: PawColor.onAction
        case .secondary: PawColor.label
        case .destructive: PawColor.danger
        }
    }

    private var ground: Color {
        switch kind {
        case .primary: PawColor.action
        case .secondary: PawColor.fill
        case .destructive: PawColor.dangerFill
        }
    }
}

extension ButtonStyle where Self == PawButtonStyle {
    static var pawPrimary: PawButtonStyle { PawButtonStyle(kind: .primary) }
    static var pawSecondary: PawButtonStyle { PawButtonStyle(kind: .secondary) }
    static var pawDestructive: PawButtonStyle { PawButtonStyle(kind: .destructive) }
}
