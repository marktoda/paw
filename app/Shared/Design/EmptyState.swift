import SwiftUI

/// Nothing to show: a slate chip, a rounded title, one or two sentences and
/// up to two buttons.
struct EmptyState: View {
    struct Action {
        let title: String
        let identifier: String
        let run: () -> Void
    }

    let symbol: String
    let title: String
    let message: String
    var primary: Action? = nil
    var secondary: Action? = nil

    var body: some View {
        ContentUnavailableView {
            Label {
                Text(title)
                    .font(PawFont.columnTitle)
                    .foregroundStyle(PawColor.label)
            } icon: {
                IconChip(symbol: symbol, tint: .slate, size: .large)
                    .padding(.bottom, PawMetrics.Spacing.xs)
            }
        } description: {
            // The title often says it all: no second line then.
            if !message.isEmpty {
                Text(message)
                    .font(PawFont.subheadline)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        } actions: {
            VStack(spacing: PawMetrics.Spacing.s) {
                if let primary {
                    Button(primary.title, action: primary.run)
                        .buttonStyle(.pawPrimary)
                        .accessibilityIdentifier(primary.identifier)
                }
                if let secondary {
                    Button(secondary.title, action: secondary.run)
                        .buttonStyle(.pawSecondary)
                        .accessibilityIdentifier(secondary.identifier)
                }
            }
            .frame(maxWidth: PawMetrics.sheetTextWidth)
        }
    }
}
