import XCTest

/// Settings › Licences: the list opens, names a crate paw links, and opens
/// its licence text. That the list covers every linked crate is checked by
/// `scripts/licences.py --check`.
final class LicencesTests: PawUITestCase {
    func testLicencesListsACrateAndOpensItsText() {
        launchFixture()
        openSettings()
        tap("settings-licences")
        expect("licence-paw")
        tap("licence-base64")
        expectText("licence-expression", contains: "MIT")
        expect("licence-text-0")
    }
}
