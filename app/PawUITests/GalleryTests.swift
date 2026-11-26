import XCTest

/// The component gallery where it is the cheapest way to a rule: the PIN
/// stage of the key sheet on its own, and the app's Home (on the fixture
/// store) at the largest text size.
final class GalleryTests: PawUITestCase {
    // MARK: Helpers

    private func launch(page: String, extra: [String] = []) {
        app.launchArguments = [
            "-pawGallery", "1",
            "-pawGalleryPage", page,
            "-pawAppearance", "light",
        ] + extra
        app.launch()
    }

    /// Waits until the label is exactly `label`.
    private func expectLabel(_ identifier: String, _ label: String,
                             file: StaticString = #filePath, line: UInt = #line) {
        let found = expect(identifier, file: file, line: line)
        if !waitUntil(timeout: Self.patience, { found.label == label }) {
            XCTFail("label of \"\(identifier)\" should be \"\(label)\", it was \"\(found.label)\"",
                    file: file, line: line)
        }
    }

    // MARK: The key sheet

    func testPinStageNeedsSixCharactersThenAsksForATouch() {
        launch(page: "sheet")
        tapWhenHittable(element("stage-pin"), "The PIN stage")

        let field = app.secureTextFields["pin-field"]
        XCTAssertTrue(waitToExist(field, timeout: Self.patience), "The PIN field should be a secure field")
        let button = app.buttons["pin-continue"]
        XCTAssertFalse(button.isEnabled, "Continue should be off with no PIN")

        focus(field)
        field.typeText("12345")
        XCTAssertFalse(button.isEnabled, "Continue should stay off at 5 characters")
        field.typeText("6")
        wait(for: "enabled == true", on: button, timeout: Self.patience, "Continue should turn on at 6 characters")
        snapshot("six-characters")

        tapWhenHittable(app.buttons["pin-show"], "Show")
        // Shown over the field, which stays secure: a plain field would let
        // a third-party keyboard in.
        let shown = app.descendants(matching: .any)["pin-shown"]
        XCTAssertTrue(waitToExist(shown, timeout: Self.patience), "Show should show the PIN")
        XCTAssertTrue(waitUntil(timeout: Self.patience) { shown.label == "123456" },
                      "Show should show the PIN typed, it showed \"\(shown.label)\"")
        XCTAssertTrue(app.secureTextFields["pin-field"].exists, "The PIN field should stay a secure field")
        XCTAssertFalse(app.textFields["pin-field"].exists, "Show must not swap in a plain text field")
        snapshot("pin-shown")

        tapWhenHittable(button, "Continue")
        expect("touch-prompt")
        expectLabel("key-sheet-headline", "Touch your key")
        expectGone(app.buttons["pin-continue"], "The PIN controls should be gone at Touch")

        tapWhenHittable(element("touch-prompt"), "The touch prompt")
        expectLabel("touch-hint", "Touch the key itself, not the screen.")
        snapshot("touch-after-pin")
    }

    // MARK: Home

    func testHomeAtTheLargestTextSize() {
        launch(page: "home", extra: [
            "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL",
        ])
        expect("home-title")
        snapshot("top")

        // A row keeps its name: it is still a wide, tall button whose label
        // starts with the name, not a chip with an ellipsis beside it.
        let row = scrollTo("face-id-github.com")
        XCTAssertTrue(row.label.hasPrefix("github.com"), row.label)
        XCTAssertGreaterThan(row.frame.width, 250, "The row should span the list")
        XCTAssertGreaterThan(row.frame.height, 100, "The row should grow with the text")
        app.swipeUp()
        snapshot("rows")
        app.swipeUp()
        snapshot("more")
    }
}
