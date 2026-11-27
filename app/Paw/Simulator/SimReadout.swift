import SwiftUI

#if DEBUG && targetEnvironment(simulator)
/// A value the UI tests read off the screen, drawn nearly invisible at 1
/// by 1 point. With `label` the text is the element's value; without, its
/// label.
struct SimReadout: View {
    let identifier: String
    let text: String
    var label: String? = nil

    var body: some View {
        let shown = Text(verbatim: text)
            .font(.caption2)
            .frame(width: 1, height: 1)
            .opacity(0.02)
        if let label {
            shown
                .accessibilityLabel(label)
                .accessibilityValue(text)
                .accessibilityIdentifier(identifier)
        } else {
            shown
                .accessibilityIdentifier(identifier)
        }
    }
}
#endif
