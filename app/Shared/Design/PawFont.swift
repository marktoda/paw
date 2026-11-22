import SwiftUI

/// The type scale. System fonts through Dynamic Type styles only.
/// Rounded is for titles and key-sheet numerals, never for row names or passwords.
enum PawFont {
    static let homeTitle = Font.system(.largeTitle, design: .rounded, weight: .bold)
    static let title = Font.system(.title, design: .rounded, weight: .bold)
    static let columnTitle = Font.system(.title2, design: .rounded, weight: .bold)
    static let numeral = Font.system(.title3, design: .rounded, weight: .semibold)
    static let headline = Font.headline
    static let body = Font.body
    static let subheadline = Font.subheadline
    static let footnote = Font.footnote
    static let sectionHeader = Font.footnote.weight(.semibold)
    static let caption = Font.caption
    static let password = Font.system(.body, design: .monospaced)
    static let passwordLarge = Font.system(.largeTitle, design: .monospaced, weight: .medium)
}
