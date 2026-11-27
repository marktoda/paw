import SwiftUI

/// A button title after the gold disc: this tap asks for the key (design.md
/// 2.1, rule 1).
struct KeyButtonLabel: View {
    let title: String

    var body: some View {
        HStack(spacing: PawMetrics.Spacing.s) {
            KeyDisc(size: .marker)
            Text(title)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title). Needs your key.")
    }
}
