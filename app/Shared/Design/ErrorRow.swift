import SwiftUI

/// A problem in words: what happened, then the next step, then one action.
struct ErrorRow: View {
    enum Kind {
        case error, warning
    }

    let kind: Kind
    /// What happened and what is safe, e.g. "Couldn't reach your git remote."
    let problem: String
    /// What to do, or what still works.
    let nextStep: String
    var actionTitle: String? = nil
    /// The action asks for the key, so it carries the gold disc.
    var actionNeedsKey = false
    var action: () -> Void = {}
    /// A banner stands alone on its own tinted card; a row sits in a list.
    var isBanner = false

    /// The problem is a headline with no full stop; spoken, it gets one.
    private var spoken: String {
        let ends = problem.last.map { ".?!:".contains($0) } ?? true
        return (ends ? problem : problem + ".") + (nextStep.isEmpty ? "" : " " + nextStep)
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: PawMetrics.Spacing.m) {
            Image(systemName: kind == .error ? PawSymbol.error : PawSymbol.warning)
                .font(.title3)
                .foregroundStyle(kind == .error ? PawColor.danger : PawColor.warning)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                    Text(problem)
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.label)
                    if !nextStep.isEmpty {
                        Text(nextStep)
                            .font(PawFont.footnote)
                            .foregroundStyle(PawColor.labelSecondary)
                    }
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("\(kind == .error ? "Error" : "Warning"): \(spoken)")
                .accessibilityIdentifier(kind == .error ? "error-row" : "warning-row")
                if let actionTitle {
                    Button(action: action) {
                        HStack(spacing: PawMetrics.Spacing.s) {
                            if actionNeedsKey {
                                KeyDisc(size: .marker)
                            }
                            Text(actionTitle)
                        }
                    }
                        .buttonStyle(.borderless)
                        .font(PawFont.subheadline.weight(.semibold))
                        .foregroundStyle(PawColor.label)
                        .padding(.horizontal, PawMetrics.Spacing.m)
                        .frame(minHeight: PawMetrics.pill)
                        .background(PawColor.fill, in: Capsule())
                        .padding(.top, PawMetrics.Spacing.s)
                        .accessibilityIdentifier("error-action")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, isBanner ? PawMetrics.Spacing.l : PawMetrics.Spacing.s)
        .padding(.horizontal, isBanner ? PawMetrics.Spacing.l : 0)
        .background {
            if isBanner {
                RoundedRectangle(cornerRadius: PawMetrics.cardRadius, style: .continuous)
                    .fill(kind == .error ? PawColor.dangerFill : PawColor.warningFill)
            }
        }
    }
}
