import SwiftUI

/// The two capsule buttons. A screen has at most one primary button.
struct PawButtonStyle: ButtonStyle {
    enum Kind {
        /// `onAction` on `action`: the one primary button on a screen.
        case primary
        /// `label` on `fill`.
        case secondary
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
            .foregroundStyle(kind == .primary ? PawColor.onAction : PawColor.label)
            .padding(.horizontal, PawMetrics.Spacing.l)
            .padding(.vertical, PawMetrics.Spacing.s)
            .frame(maxWidth: .infinity, minHeight: PawMetrics.control)
            .background(kind == .primary ? PawColor.action : PawColor.fill, in: Capsule())
            .contentShape(Capsule())
            .opacity(isEnabled ? (configuration.isPressed ? 0.7 : 1) : 0.4)
    }
}

extension ButtonStyle where Self == PawButtonStyle {
    static var pawPrimary: PawButtonStyle { PawButtonStyle(kind: .primary) }
    static var pawSecondary: PawButtonStyle { PawButtonStyle(kind: .secondary) }
}
