import SwiftUI

/// The eight chip tints. A tint is a chip background and a glyph colour;
/// it is never the only cue, and never gold, orange, red or green.
enum FolderTint: Int, CaseIterable {
    case blue, sky, teal, olive, purple, magenta, cocoa, slate

    /// The tint of a top-level folder: FNV-1a 32-bit of the lower-cased, NFC
    /// name, mod 7. Stable across launches and devices (Swift's `Hasher` is
    /// not). Entries in no folder pass nil and get slate.
    init(folder: String?) {
        guard let folder else {
            self = .slate
            return
        }
        var hash: UInt32 = 2_166_136_261
        for byte in folder.lowercased().precomposedStringWithCanonicalMapping.utf8 {
            hash = (hash ^ UInt32(byte)) &* 16_777_619
        }
        self = FolderTint(rawValue: Int(hash % 7)) ?? .slate
    }

    var name: String {
        switch self {
        case .blue: "blue"
        case .sky: "sky"
        case .teal: "teal"
        case .olive: "olive"
        case .purple: "purple"
        case .magenta: "magenta"
        case .cocoa: "cocoa"
        case .slate: "slate"
        }
    }

    var chip: Color { PawColor.dyn(values.chipLight, values.chipDark) }
    var glyph: Color { PawColor.dyn(values.glyphLight, values.glyphDark) }

    private var values: (chipLight: UInt32, glyphLight: UInt32, chipDark: UInt32, glyphDark: UInt32) {
        switch self {
        case .blue: (0xE7EFFE, 0x003EB5, 0x182846, 0xB9D2FF)
        case .sky: (0xDFF2FC, 0x0C8BBB, 0x152C38, 0x6DB5DA)
        case .teal: (0xE2F3F1, 0x004744, 0x182D2C, 0x8ACCC7)
        case .olive: (0xEDF1E0, 0x7B8935, 0x272B18, 0xB9C588)
        case .purple: (0xF2EAFF, 0x52008D, 0x2E2140, 0xA981E1)
        case .magenta: (0xFFE7F2, 0xC94893, 0x3C1D2E, 0xD473A7)
        case .cocoa: (0xF5EDE8, 0x886650, 0x302721, 0xC0A593)
        case .slate: (0xEDEEF1, 0x595D69, 0x28292C, 0xE0E4EF)
        }
    }
}
