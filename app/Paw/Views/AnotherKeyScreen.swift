import SwiftUI

/// An entry the plugged-in key cannot open. Says which keys can, worked
/// out from the file alone: no PIN, no touch.
struct AnotherKeyScreen: View {
    let model: AppModel
    let entry: EntryRef

    var body: some View {
        List {
            Section {
                HStack(spacing: 14) {
                    IconChip(symbol: EntryGlyph.symbol(for: entry.name), tint: FolderTint(folder: entry.topFolder), size: .large)
                    VStack(alignment: .leading, spacing: 0) {
                        Text(entry.name)
                            .font(PawFont.title)
                            .foregroundStyle(PawColor.label)
                            .accessibilityAddTraits(.isHeader)
                            .accessibilityIdentifier("entry-title")
                        Text(entry.folder.isEmpty ? "Not in a folder" : entry.folder)
                            .font(PawFont.subheadline)
                            .foregroundStyle(PawColor.labelSecondary)
                    }
                }
                .padding(.horizontal, PawMetrics.Spacing.xs)
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            }
            Section {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.xs) {
                    Text("Encrypted for another key")
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

    private var recipients: [Recipient] {
        if case .notForThisKey(let recipients) = model.accessByPath[entry.path] {
            return recipients
        }
        return []
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
