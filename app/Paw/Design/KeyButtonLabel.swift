import SwiftUI

/// A primary button whose tap asks for the key: the gold disc leads it
/// (design.md 2.1, rule 1).
struct KeyButtonLabel: View {
    let title: String

    var body: some View {
        HStack(spacing: PawMetrics.Spacing.s) {
            KeyDisc(size: .marker)
            Text(title)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title), needs your key")
    }
}
