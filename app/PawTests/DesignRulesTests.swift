import UIKit
import XCTest
@testable import Paw

/// Rules of the design components that every screen relies on: a folder
/// keeps its tint, and every symbol the app names exists.
final class DesignRulesTests: XCTestCase {
    func testFolderTintsAreStable() {
        // FNV-1a 32-bit of the lower-cased, NFC name, mod 7, worked out
        // independently of the app.
        let expected: [(folder: String, tint: String)] = [
            ("bank", "cocoa"), ("Bank", "cocoa"), ("dev", "blue"), ("email", "sky"), ("home", "teal"),
            ("shopping", "olive"), ("work", "sky"), ("travel", "purple"), ("school", "magenta"),
            ("caf\u{00E9}", "olive"), ("cafe\u{0301}", "olive"),
        ]
        for (folder, tint) in expected {
            XCTAssertEqual(FolderTint(folder: folder).name, tint, folder)
        }
        XCTAssertEqual(FolderTint(folder: nil).name, "slate", "No folder")
    }

    func testEverySymbolExists() {
        // A missing symbol draws nothing, with no error.
        let names = PawSymbol.all.map(\.name) + EntryGlyph.table.map(\.symbol)
        XCTAssertEqual(names.filter { UIImage(systemName: $0) == nil }, [])
    }
}
