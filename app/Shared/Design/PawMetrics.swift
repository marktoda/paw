import SwiftUI

/// Sizes, spacing and radii. See docs/design-language.md, section 4.
enum PawMetrics {
    enum Spacing {
        static let xxs: CGFloat = 2
        static let xs: CGFloat = 4
        static let s: CGFloat = 8
        static let m: CGFloat = 12
        static let l: CGFloat = 16
        static let xl: CGFloat = 24
        static let xxl: CGFloat = 32
    }

    static let chip: CGFloat = 30
    static let chipGlyph: CGFloat = 16
    static let chipLarge: CGFloat = 52
    static let chipLargeGlyph: CGFloat = 26
    static let chipRadiusRatio: CGFloat = 0.27

    static let discMarker: CGFloat = 14
    static let discBanner: CGFloat = 36
    static let discHero: CGFloat = 92

    static let rowOneLine: CGFloat = 48
    static let rowTwoLine: CGFloat = 56
    static let rowField: CGFloat = 64
    static let control: CGFloat = 50
    static let tapMin: CGFloat = 44
    static let pill: CGFloat = 30

    static let inlineRadius: CGFloat = 14
    static let cardRadius: CGFloat = 20

    static let sheetTextWidth: CGFloat = 290
    static let readableWidth: CGFloat = 560
    static let keySheetHeight: CGFloat = 480

    /// Chips and small discs grow with Dynamic Type, but only this far, so
    /// that at the largest sizes the width goes to the text.
    static let maxFixedScale: CGFloat = 1.5
}
