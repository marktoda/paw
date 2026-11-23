import SwiftUI

/// The copy confirmation: a capsule with a check and one sentence.
/// A secret says when it clears: "Password copied. Clears in 45 seconds."
struct CopyToast: View {
    let message: String

    var body: some View {
        HStack(spacing: PawMetrics.Spacing.s) {
            Image(systemName: PawSymbol.synced)
                .font(PawFont.subheadline)
            Text(message)
                .font(PawFont.subheadline.weight(.semibold))
                .multilineTextAlignment(.leading)
        }
        .foregroundStyle(PawColor.onPanel)
        .padding(.horizontal, PawMetrics.Spacing.l)
        .padding(.vertical, PawMetrics.Spacing.m)
        .frame(minHeight: PawMetrics.tapMin)
        .background(PawColor.panel, in: Capsule())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(message)
        .accessibilityIdentifier("copy-toast")
    }
}

private struct CopyToastModifier: ViewModifier {
    @Binding var message: String?
    let seconds: Double

    func body(content: Content) -> some View {
        content
            .overlay(alignment: .bottom) {
                if let message {
                    CopyToast(message: message)
                        .padding(.horizontal, PawMetrics.Spacing.l)
                        .padding(.bottom, PawMetrics.Spacing.xl)
                        .transition(.opacity)
                        // Spoken as an announcement instead; not focusable.
                        .accessibilityHidden(true)
                        // A tap on the button under it still reaches the button.
                        .allowsHitTesting(false)
                }
            }
            .animation(.easeOut(duration: 0.2), value: message)
            .sensoryFeedback(.impact(weight: .light), trigger: message) { _, new in
                new != nil
            }
            .task(id: message) {
                guard let shown = message else { return }
                AccessibilityNotification.Announcement(shown).post()
                try? await Task.sleep(for: .seconds(seconds))
                if !Task.isCancelled {
                    message = nil
                }
            }
    }
}

extension View {
    /// Shows a copy confirmation over the bottom of this view for 2 seconds
    /// whenever `message` is set, with a light tap and a spoken announcement.
    func copyToast(_ message: Binding<String?>, seconds: Double = 2) -> some View {
        modifier(CopyToastModifier(message: message, seconds: seconds))
    }
}
