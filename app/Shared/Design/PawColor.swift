import SwiftUI
import UIKit

/// Every colour paw draws with. See docs/design-language.md, section 2.
/// Gold means the key and nothing else: `keyGold` is only ever used by `KeyDisc`.
enum PawColor {
    static let bg = Color(uiColor: .systemGroupedBackground)
    static let surface = Color(uiColor: .secondarySystemGroupedBackground)
    static let fill = Color(uiColor: .tertiarySystemFill)
    static let label = Color.primary

    // Custom ink. The system secondary label is too faint on the grouped background.
    static let labelSecondary = Color(uiColor: UIColor { traits in
        let highContrast = traits.accessibilityContrast == .high
        if traits.userInterfaceStyle == .dark {
            return UIColor(hex: highContrast ? 0xC9C8D6 : 0xA9A8B6)
        }
        return UIColor(hex: highContrast ? 0x3F3E4C : 0x5C5C66)
    })
    static let labelTertiary = dyn(0x8B8A96, 0x7E7D8A)
    static let action = dyn(0x1B1A2B, 0xF1F0F7)
    static let onAction = dyn(0xFFFFFF, 0x16151F)
    static let panel = dyn(0x1B1A2B, 0x26243A)
    static let onPanel = Color(hex: 0xFFFFFF)

    // The key disc only.
    static let keyGold = Color(hex: 0xF5B301)
    static let keyGoldRim = Color(hex: 0xB26A00)
    static let keyGoldHalo = Color(hex: 0xF5B301).opacity(0.24)
    static let keyGoldHaloOuter = Color(hex: 0xF5B301).opacity(0.12)

    // Status. Always shown with a glyph or a word.
    static let success = dyn(0x005729, 0x81F0A3)
    static let warning = dyn(0xAE4609, 0xF99770)
    static let danger = dyn(0xA7002E, 0xF56B76)
    static let warningFill = dyn(0xFFEADE, 0x381F11)
    static let dangerFill = dyn(0xFEE9E9, 0x3B1C1D)

    // Password characters.
    static let passwordDigit = dyn(0x0038AB, 0x86B1FF)
    static let passwordSymbol = dyn(0xBB378D, 0xFEBEE1)

    static func dyn(_ light: UInt32, _ dark: UInt32) -> Color {
        Color(uiColor: UIColor { traits in
            UIColor(hex: traits.userInterfaceStyle == .dark ? dark : light)
        })
    }
}

extension UIColor {
    /// An opaque sRGB colour from 0xRRGGBB.
    convenience init(hex: UInt32) {
        self.init(
            red: CGFloat((hex >> 16) & 0xFF) / 255,
            green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: 1)
    }
}

extension Color {
    /// An opaque sRGB colour from 0xRRGGBB, the same in light and dark.
    init(hex: UInt32) {
        self.init(uiColor: UIColor(hex: hex))
    }
}
