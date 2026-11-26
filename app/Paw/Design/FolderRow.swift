import SwiftUI

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
        .accessibilityLabel("\(name), folder, \(passwordCount(count))")
        .accessibilityIdentifier("folder-\(name)")
    }
}
