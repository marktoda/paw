import SwiftUI

/// An entry the plugged-in key cannot open. Says which keys can, worked
/// out from the file alone: no PIN, no touch.
struct AnotherKeyScreen: View {
    let entry: EntryRef
    let recipients: [Recipient]

    /// Also what its row says in a list.
    static let headline = "Encrypted for another key"

    var body: some View {
        List {
            Section {
                EntryHeader(entry: entry)
                    .listRowInsets(EdgeInsets())
                    .listRowBackground(Color.clear)
            }
            Section {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.xs) {
                    Text(Self.headline)
                        .font(PawFont.headline)
                        .foregroundStyle(PawColor.label)
                    Text(explanation)
                        .font(PawFont.subheadline)
                        .foregroundStyle(PawColor.labelSecondary)
                }
                .padding(.vertical, PawMetrics.Spacing.m)
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("another-key-explanation")
                .pawListRow()
            }
            if !keys.isEmpty {
                Section {
                    ForEach(recipients.map(\.grouped), id: \.self) { key in
                        Text(key)
                            .font(PawFont.password)
                            .foregroundStyle(PawColor.label)
                            .textSelection(.enabled)
                            .frame(minHeight: PawMetrics.rowOneLine)
                            .pawListRow()
                    }
                } header: {
                    PawSectionHeader(keys.count == 1 ? "That key" : "Those keys")
                } footer: {
                    Text(footer)
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                        .accessibilityIdentifier("another-key-footer")
                }
            }
        }
        .listStyle(.insetGrouped)
        .contentMargins(.top, 0, for: .scrollContent)
        .navigationBarTitleDisplayMode(.inline)
    }

    private var keys: [String] {
        recipients.map(\.spoken)
    }

    private var footer: String {
        let which = keys.count == 1 ? "that key" : "one of these keys"
        return "Open it on a computer that has \(which)."
    }

    private var explanation: String {
        let names = keys.joined(separator: ", ")
        return names.isEmpty ? "Your key can't open it." : "Only \(names) can open it."
    }
}
