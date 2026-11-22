import XCTest

/// Opens each page of the component gallery in light and in dark, checks
/// that the components are there and behave, and leaves pictures of them.
final class GalleryTests: XCTestCase {
    private var app: XCUIApplication!

    private static let appearances = ["light", "dark"]
    private static let samplePassword = "Xk7#mO0l1-pT9&wq"

    override func setUp() {
        continueAfterFailure = false
        app = XCUIApplication()
    }

    // MARK: Helpers

    private func launch(page: String, appearance: String, extra: [String] = []) {
        app.launchArguments = [
            "-pawGallery", "1",
            "-pawGalleryPage", page,
            "-pawAppearance", appearance,
        ] + extra
        app.launch()
    }

    private func snapshot(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = "\(self.name.split(separator: " ").last?.dropLast() ?? "test")-\(name)"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    /// The first element with this identifier, whatever its type.
    private func element(_ identifier: String) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: identifier).firstMatch
    }

    /// Waits for an element and fails with its identifier otherwise.
    @discardableResult
    private func expect(_ identifier: String, timeout: TimeInterval = 10,
                        file: StaticString = #filePath, line: UInt = #line) -> XCUIElement {
        let found = element(identifier)
        XCTAssertTrue(found.waitForExistence(timeout: timeout),
                      "\"\(identifier)\" should be on screen", file: file, line: line)
        return found
    }

    private func expectLabel(_ identifier: String, _ label: String,
                             file: StaticString = #filePath, line: UInt = #line) {
        let found = expect(identifier, file: file, line: line)
        XCTAssertEqual(found.label, label, "label of \"\(identifier)\"", file: file, line: line)
    }

    // MARK: Tokens

    func testColoursPage() {
        for appearance in Self.appearances {
            launch(page: "colours", appearance: appearance)
            expect("swatch-bg")
            expect("swatch-keyGold")
            snapshot(appearance)
            app.swipeUp()
            expect("swatch-passwordSymbol")
            snapshot("\(appearance)-more")
        }
    }

    func testTypePage() {
        for appearance in Self.appearances {
            launch(page: "type", appearance: appearance)
            expect("type-homeTitle")
            expect("type-password")
            snapshot(appearance)
        }
    }

    func testFolderTintsAreStable() {
        // FNV-1a 32-bit of the lower-cased, NFC name, mod 7, worked out
        // independently of the app. The page lists them in this order.
        let expected = "bank cocoa, Bank cocoa, dev blue, email sky, home teal, shopping olive, "
            + "work sky, travel purple, school magenta, cafe-composed olive, cafe-decomposed olive"
        for appearance in Self.appearances {
            launch(page: "tints", appearance: appearance)
            expect("tint-blue")
            expect("tint-slate")
            snapshot(appearance)
            if appearance == "light" {
                expectLabel("tint-assignments", expected)
                expectLabel("tint-of-none", "slate")
            }
            app.swipeUp()
            snapshot("\(appearance)-assignment")
        }
    }

    func testEverySymbolExists() {
        for appearance in Self.appearances {
            launch(page: "symbols", appearance: appearance)
            expectLabel("symbols-missing", "0 missing")
            snapshot(appearance)
            app.swipeUp()
            snapshot("\(appearance)-more")
        }
    }

    // MARK: Components

    func testRowsPage() {
        for appearance in Self.appearances {
            launch(page: "rows", appearance: appearance)
            expectLabel("entry-GitHub", "GitHub, dev, opens with Face ID")
            expectLabel("entry-Fastmail", "Fastmail, email, needs your key")
            XCTAssertTrue(app.buttons["entry-Fastmail"].exists, "An entry row should be a button")
            snapshot(appearance)

            app.buttons["entry-GitHub"].tap()
            expectLabel("rows-last-tapped", "Tapped GitHub")

            app.swipeUp()
            expectLabel("folder-bank", "bank, folder, 9 passwords")
            expectLabel("folder-spare", "spare, folder, 1 password")
            snapshot("\(appearance)-folders")
            app.swipeUp()
            expect("error-row")
            expect("warning-row")
            snapshot("\(appearance)-problems")
        }
    }

    func testKeyDiscPage() {
        for appearance in Self.appearances {
            launch(page: "disc", appearance: appearance)
            expectLabel("disc-marker", "Needs your key")
            expectLabel("disc-locked", "Your key is locked")
            snapshot(appearance)
            app.swipeUp()
            snapshot("\(appearance)-grounds")
        }
    }

    func testKeySheetStages() {
        let headlines = [
            ("plugIn", "Plug in your key"),
            ("pin", "Enter your key's PIN"),
            ("pinForgotten", "Enter your key's PIN"),
            ("pinLastTry", "Enter your key's PIN"),
            ("wrongPin", "Wrong PIN"),
            ("busy", "One moment"),
            ("touch", "Touch your key"),
            ("working", "Opening Fastmail\u{2026}"),
            ("locked", "Your key is locked"),
        ]
        for appearance in Self.appearances {
            launch(page: "sheet", appearance: appearance)
            for (stage, headline) in headlines {
                expect("stage-\(stage)").tap()
                expectLabel("key-sheet-headline", headline)
                switch stage {
                case "pinLastTry":
                    expectLabel("pin-tries-left", "1 try left before the key locks.")
                    XCTAssertEqual(app.buttons["pin-continue"].label, "Use last try")
                    XCTAssertTrue(app.textFields["pin-field"].exists,
                                  "With one try left the PIN is shown from the start")
                case "wrongPin":
                    expectLabel("pin-tries-left", "2 tries left before the key locks.")
                case "touch":
                    expect("touch-prompt")
                    expect("key-sheet-note")
                default:
                    break
                }
                snapshot("\(appearance)-\(stage)")
                let cancel = app.buttons["key-sheet-cancel"]
                cancel.tap()
                XCTAssertTrue(cancel.waitForNonExistence(timeout: 10),
                              "Cancel should close the sheet at \(stage)")
            }

            // Done has no button: it closes itself.
            expect("stage-done").tap()
            let done = app.staticTexts["key-sheet-headline"]
            XCTAssertTrue(done.waitForExistence(timeout: 3), "Done should show before the sheet closes")
            snapshot("\(appearance)-done")
            XCTAssertTrue(done.waitForNonExistence(timeout: 10),
                          "The sheet should close itself after Done")
        }
    }

    func testPinStageNeedsSixCharactersThenAsksForATouch() {
        launch(page: "sheet", appearance: "light")
        expect("stage-pin").tap()

        let field = app.secureTextFields["pin-field"]
        XCTAssertTrue(field.waitForExistence(timeout: 10), "The PIN field should be a secure field")
        let button = app.buttons["pin-continue"]
        XCTAssertFalse(button.isEnabled, "Continue should be off with no PIN")

        field.tap()
        field.typeText("12345")
        XCTAssertFalse(button.isEnabled, "Continue should stay off at 5 characters")
        field.typeText("6")
        XCTAssertTrue(button.isEnabled, "Continue should turn on at 6 characters")
        snapshot("six-characters")

        app.buttons["pin-show"].tap()
        let shown = app.textFields["pin-field"]
        XCTAssertTrue(shown.waitForExistence(timeout: 5), "Show should turn the PIN into plain text")
        XCTAssertEqual(shown.value as? String, "123456")
        snapshot("pin-shown")

        button.tap()
        expect("touch-prompt")
        expectLabel("key-sheet-headline", "Touch your key")
        XCTAssertFalse(app.buttons["pin-continue"].exists, "The PIN controls should be gone at Touch")

        element("touch-prompt").tap()
        expectLabel("touch-hint", "Touch the key itself, not the screen.")
        snapshot("touch-after-pin")
    }

    func testEntryPagePasswordField() {
        for appearance in Self.appearances {
            launch(page: "entry", appearance: appearance)
            expect("entry-title")
            expect("face-id-window-text")
            let value = expect("password-value")
            XCTAssertEqual(value.value as? String, "hidden", "The password should start hidden")
            snapshot("\(appearance)-hidden")

            app.buttons["password-reveal"].tap()
            XCTAssertEqual(element("password-value").value as? String, Self.samplePassword,
                           "Reveal should show the password")
            snapshot("\(appearance)-revealed")

            app.buttons["password-large-type"].tap()
            expectLabel("large-type-cell-1", "Capital X, 1 of 16")
            expectLabel("large-type-cell-4", "Hash, 4 of 16")
            expectLabel("large-type-cell-7", "0, 7 of 16")
            snapshot("\(appearance)-large-type")
            let done = app.buttons["large-type-done"]
            done.tap()
            XCTAssertTrue(done.waitForNonExistence(timeout: 10), "Done should close the large-type view")

            app.buttons["password-copy"].tap()
            snapshot("\(appearance)-copied")

            app.buttons["password-reveal"].tap()
            XCTAssertEqual(element("password-value").value as? String, "hidden",
                           "The eye should hide the password again")

            app.buttons["lock-now"].tap()
            XCTAssertTrue(element("face-id-window-text").waitForNonExistence(timeout: 5),
                          "Lock now should remove the Face ID window bar")
        }
    }

    func testStatusPage() {
        for appearance in Self.appearances {
            launch(page: "status", appearance: appearance)
            expectLabel("sync-pill", "Sync status: Synced 2 minutes ago")
            expectLabel("unpushed-banner",
                        "1 change is only on this iPhone. Plug in your key to send it. Needs your key.")
            expect("face-id-window-text")
            expectLabel("copy-toast", "Password copied. Clears in 45 seconds.")
            expectLabel("error-row",
                        "Error: Couldn't reach your git remote. Passwords on this iPhone still work.")
            snapshot(appearance)
            app.swipeUp()
            snapshot("\(appearance)-more")
        }
    }

    func testEmptyStatesPage() {
        for appearance in Self.appearances {
            launch(page: "empty", appearance: appearance)
            XCTAssertTrue(app.staticTexts["No passwords yet"].waitForExistence(timeout: 10))
            XCTAssertTrue(app.staticTexts["No matches"].exists)
            snapshot(appearance)
            app.buttons["empty-add"].tap()
            expectLabel("empty-last-tapped", "Tapped Add a password")
            app.swipeUp()
            snapshot("\(appearance)-more")
        }
    }

    // MARK: Home

    func testHomeMock() {
        for appearance in Self.appearances {
            launch(page: "home", appearance: appearance)
            expectLabel("home-title", "Passwords")
            expect("sync-pill")
            expect("unpushed-banner")
            expectLabel("entry-GitHub", "GitHub, dev, opens with Face ID")
            expectLabel("entry-Fastmail", "Fastmail, email, needs your key")
            expectLabel("add-button", "Add a password")
            expect("settings-button")
            snapshot(appearance)

            let search = app.searchFields.firstMatch
            XCTAssertTrue(search.waitForExistence(timeout: 5), "Home should have a search field")
            search.tap()
            snapshot("\(appearance)-search-empty")
            search.typeText("fast")
            expectLabel("search-count", "4 matches")
            expect("entry-Breakfast delivery club")
            XCTAssertFalse(app.buttons["entry-GitHub"].exists, "GitHub does not match \"fast\"")
            snapshot("\(appearance)-search")
        }
    }

    func testHomeFolders() {
        for appearance in Self.appearances {
            launch(page: "home", appearance: appearance)
            expect("home-title")
            app.swipeUp()
            expectLabel("folder-bank", "bank, folder, 9 passwords")
            expect("folder-shopping")
            snapshot(appearance)
        }
    }

    func testHomeAtTheLargestTextSize() {
        launch(page: "home", appearance: "light", extra: [
            "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL",
        ])
        expect("home-title")
        snapshot("top")

        // A row keeps its name: it is still a wide, tall button whose label
        // starts with the name, not a chip with an ellipsis beside it.
        let row = expect("entry-GitHub")
        XCTAssertTrue(row.label.hasPrefix("GitHub"), row.label)
        XCTAssertGreaterThan(row.frame.width, 250, "The row should span the list")
        XCTAssertGreaterThan(row.frame.height, 100, "The row should grow with the text")
        app.swipeUp()
        snapshot("rows")
        app.swipeUp()
        snapshot("more")
    }
}
