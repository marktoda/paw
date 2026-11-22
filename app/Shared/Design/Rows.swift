import SwiftUI

/// One password in a list: chip, name, subtitle, and a trailing mark that
/// says how it opens. The gold disc appears only when the tap needs the key.
struct EntryRow: View {
    enum Access: Equatable {
        /// Opens with Face ID: no key needed for this tap.
        case faceID
        /// The next tap asks for the key.
        case needsKey
    }

    let name: String
    let subtitle: String
    var symbol: String = PawSymbol.entry
    var tint: FolderTint = .slate
    let access: Access
    /// A search term; the part of the name that matches is drawn semibold.
    var match: String? = nil
    var action: () -> Void = {}

    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        Button(action: action) {
            HStack(spacing: PawMetrics.Spacing.m) {
                IconChip(symbol: symbol, tint: tint)
                VStack(alignment: .leading, spacing: 0) {
                    Text(displayName)
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.label)
                        .lineLimit(typeSize.isAccessibilitySize ? 3 : 1)
                        .truncationMode(.tail)
                    Text(subtitle)
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                        .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                switch access {
                case .faceID:
                    Image(systemName: PawSymbol.faceID)
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.labelSecondary)
                case .needsKey:
                    KeyDisc(size: .marker)
                        .padding(.trailing, 3)
                }
            }
            .padding(.vertical, PawMetrics.Spacing.s)
            .frame(minHeight: PawMetrics.rowTwoLine)
            .contentShape(Rectangle())
        }
        .accessibilityLabel(spokenLabel)
        .accessibilityIdentifier("entry-\(name)")
    }

    private var displayName: AttributedString {
        var text = AttributedString(name)
        if let match, !match.isEmpty,
           let range = text.range(of: match, options: [.caseInsensitive, .diacriticInsensitive]) {
            text[range].font = Font.body.weight(.semibold)
        }
        return text
    }

    private var spokenLabel: String {
        switch access {
        case .faceID: "\(name), \(subtitle), opens with Face ID"
        case .needsKey: "\(name), \(subtitle), needs your key"
        }
    }
}

/// One folder in a list: tinted folder chip, name, count, chevron.
struct FolderRow: View {
    let name: String
    let count: Int
    var tint: FolderTint? = nil
    var action: () -> Void = {}

    var body: some View {
        Button(action: action) {
            HStack(spacing: PawMetrics.Spacing.m) {
                IconChip(symbol: PawSymbol.folder, tint: tint ?? FolderTint(folder: name))
                Text(name)
                    .font(PawFont.body)
                    .foregroundStyle(PawColor.label)
                    .lineLimit(2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                Text(count, format: .number)
                    .font(PawFont.body.monospacedDigit())
                    .foregroundStyle(PawColor.labelSecondary)
                Image(systemName: PawSymbol.disclosure)
                    .font(PawFont.footnote.weight(.semibold))
                    .foregroundStyle(PawColor.labelTertiary)
            }
            .padding(.vertical, PawMetrics.Spacing.s)
            .frame(minHeight: PawMetrics.rowOneLine)
            .contentShape(Rectangle())
        }
        .accessibilityLabel("\(name), folder, \(count) \(count == 1 ? "password" : "passwords")")
        .accessibilityIdentifier("folder-\(name)")
    }
}

/// A section title: footnote semibold, sentence case, read as a heading.
struct PawSectionHeader: View {
    let title: String

    init(_ title: String) {
        self.title = title
    }

    var body: some View {
        Text(title)
            .font(PawFont.sectionHeader)
            .foregroundStyle(PawColor.labelSecondary)
            .textCase(nil)
            .accessibilityAddTraits(.isHeader)
    }
}

extension View {
    /// Gives a paw row the full height it asks for inside a `List`: the row
    /// sets its own vertical padding, the list only keeps the 16pt margins.
    func pawListRow() -> some View {
        listRowInsets(EdgeInsets(
            top: 0, leading: PawMetrics.Spacing.l, bottom: 0, trailing: PawMetrics.Spacing.l))
    }
}
