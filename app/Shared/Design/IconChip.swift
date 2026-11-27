import SwiftUI

/// A tinted rounded square with a glyph, on rows (`regular`) and in
/// headers (`large`).
/// Decorative: the row or header it sits in carries the words.
struct IconChip: View {
    enum Size {
        case regular, large

        var side: CGFloat { self == .regular ? PawMetrics.chip : PawMetrics.chipLarge }
        var glyph: CGFloat { self == .regular ? PawMetrics.chipGlyph : PawMetrics.chipLargeGlyph }
    }

    let symbol: String
    var tint: FolderTint = .slate
    var size: Size = .regular

    @ScaledMetric(relativeTo: .body) private var scale: CGFloat = 1

    var body: some View {
        let factor = min(max(scale, 1), PawMetrics.maxFixedScale)
        let side = size.side * factor
        RoundedRectangle(cornerRadius: side * PawMetrics.chipRadiusRatio, style: .continuous)
            .fill(tint.chip)
            .frame(width: side, height: side)
            .overlay {
                Image(systemName: symbol)
                    .font(.system(size: size.glyph * factor, weight: .semibold))
                    .foregroundStyle(tint.glyph)
            }
            .accessibilityHidden(true)
    }
}
