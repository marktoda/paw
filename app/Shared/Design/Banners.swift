import SwiftUI

/// "Not yet pushed": changes that are only on this iPhone. Not dismissable.
struct UnpushedBanner: View {
    enum Reason {
        /// Sending needs the key: the banner carries the gold disc.
        case waitingForKey
        case offline
        /// Saved here, nothing tried yet: no colour, `arrow.up.circle`.
        case waiting
        /// The git remote refused the push. The only state that is a warning.
        case refused
    }

    let reason: Reason
    let title: String
    let detail: String
    var action: () -> Void = {}

    var body: some View {
        Button(action: action) {
            HStack(spacing: 14) {
                mark
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                    Text(title)
                        .font(PawFont.headline)
                        .foregroundStyle(reason == .waitingForKey ? PawColor.onPanel : PawColor.label)
                    Text(detail)
                        .font(PawFont.footnote)
                        .foregroundStyle(
                            reason == .waitingForKey ? PawColor.onPanelSecondary : PawColor.labelSecondary)
                }
                .multilineTextAlignment(.leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.horizontal, PawMetrics.Spacing.l)
            .padding(.vertical, PawMetrics.Spacing.m)
            .frame(minHeight: PawMetrics.rowField)
            .background(ground, in: RoundedRectangle(cornerRadius: PawMetrics.cardRadius, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: PawMetrics.cardRadius, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(spokenLabel)
        .accessibilityIdentifier("unpushed-banner")
    }

    @ViewBuilder private var mark: some View {
        switch reason {
        case .waitingForKey:
            KeyDisc(size: .banner)
        case .waiting:
            Image(systemName: PawSymbol.notPushed)
                .font(.title2)
                .foregroundStyle(PawColor.label)
                .frame(minWidth: PawMetrics.discBanner)
        case .offline:
            Image(systemName: PawSymbol.offline)
                .font(.title2)
                .foregroundStyle(PawColor.label)
                .frame(minWidth: PawMetrics.discBanner)
        case .refused:
            Image(systemName: PawSymbol.warning)
                .font(.title2)
                .foregroundStyle(PawColor.warning)
                .frame(minWidth: PawMetrics.discBanner)
        }
    }

    private var ground: Color {
        switch reason {
        case .waitingForKey: PawColor.panel
        case .offline, .waiting: PawColor.surface
        case .refused: PawColor.warningFill
        }
    }

    private var spokenLabel: String {
        let words = "\(title). \(detail)"
        return reason == .waitingForKey ? "\(words) Needs your key." : words
    }
}

/// Says the Face ID window is open and until when, with one tap to close it.
/// No colour: green would claim protection it does not have.
struct FaceIDWindowBar: View {
    /// e.g. "Opens with Face ID until 6:40 PM"
    let text: String
    var onLock: () -> Void = {}

    @State private var locks = 0

    var body: some View {
        HStack(spacing: PawMetrics.Spacing.s) {
            Image(systemName: PawSymbol.faceID)
                .font(PawFont.subheadline)
                .foregroundStyle(PawColor.label)
                .accessibilityHidden(true)
            Text(text)
                .font(PawFont.subheadline)
                .foregroundStyle(PawColor.label)
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityIdentifier("face-id-window-text")
            Button("Lock now") {
                locks += 1
                onLock()
            }
            .buttonStyle(.borderless)
            .font(PawFont.subheadline.weight(.semibold))
            .foregroundStyle(PawColor.label)
            .frame(minHeight: PawMetrics.tapMin)
            .accessibilityIdentifier("lock-now")
        }
        .padding(.horizontal, PawMetrics.Spacing.l)
        .frame(minHeight: PawMetrics.tapMin)
        .background(
            PawColor.fill,
            in: RoundedRectangle(cornerRadius: PawMetrics.inlineRadius, style: .continuous))
        .sensoryFeedback(.impact(flexibility: .rigid), trigger: locks)
    }
}
