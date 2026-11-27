import SwiftUI

/// Nothing to show: a slate chip, a rounded title, one or two sentences and
/// a button.
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

    var body: some View {
        ContentUnavailableView {
            Label {
                Text(Device.words(title))
                    .font(PawFont.columnTitle)
                    .foregroundStyle(PawColor.label)
            } icon: {
                IconChip(symbol: symbol, tint: .slate, size: .large)
                    .padding(.bottom, PawMetrics.Spacing.xs)
            }
        } description: {
            // The title often says it all: no second line then.
            if !message.isEmpty {
                Text(Device.words(message))
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
            }
            .frame(maxWidth: PawMetrics.sheetTextWidth)
        }
    }
}
