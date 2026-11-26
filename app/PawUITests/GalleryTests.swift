import XCTest

/// Opens each page of the component gallery in light and in dark, checks
/// that the components are there and behave, and leaves pictures of them.
final class GalleryTests: PawUITestCase {
    private static let appearances = ["light", "dark"]
    /// bank/first-example-bank's, in the fixture store the entry page opens.
    private static let samplePassword = "Harbor_73_willow"

    // MARK: Helpers

    private func launch(page: String, appearance: String, extra: [String] = []) {
        app.launchArguments = [
            "-pawGallery", "1",
            "-pawGalleryPage", page,
            "-pawAppearance", appearance,
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

            tapWhenHittable(app.buttons["entry-GitHub"], "The GitHub row")
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
                tapWhenHittable(element("stage-\(stage)"), "The \(stage) stage")
                expectLabel("key-sheet-headline", headline)
                switch stage {
                case "pinLastTry":
                    expectLabel("pin-tries-left", "1 try left before the key locks.")
                    XCTAssertEqual(app.buttons["pin-continue"].label, "Use last try")
                    XCTAssertEqual(app.buttons["pin-show"].label, "Hide PIN",
                                   "With one try left the PIN is shown from the start")
                    XCTAssertTrue(app.secureTextFields["pin-field"].exists,
                                  "Shown or not, the PIN field stays a secure field")
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
                tapWhenHittable(cancel, "Cancel at \(stage)")
                expectGone(cancel, "Cancel should close the sheet at \(stage)")
            }

            // Done has no button: it closes itself.
            tapWhenHittable(element("stage-done"), "The done stage")
            let done = app.staticTexts["key-sheet-headline"]
            // The gallery closes it after 4 seconds.
            XCTAssertTrue(waitToExist(done, timeout: 3), "Done should show before the sheet closes")
            snapshot("\(appearance)-done")
            expectGone(done, "The sheet should close itself after Done")
        }
    }

    func testPinStageNeedsSixCharactersThenAsksForATouch() {
        launch(page: "sheet", appearance: "light")
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

    func testEntryPagePasswordField() {
        for appearance in Self.appearances {
            launch(page: "entry", appearance: appearance)
            expectLabel("entry-title", "first-example-bank")
            expect("face-id-window-text")
            let value = expect("password-value")
            XCTAssertEqual(value.value as? String, "hidden", "The password should start hidden")
            snapshot("\(appearance)-hidden")

            tapWhenHittable(app.buttons["password-reveal"], "The eye")
            expectValue("password-value", Self.samplePassword)
            snapshot("\(appearance)-revealed")

            tapWhenHittable(app.buttons["password-large-type"], "Large type")
            expectLabel("large-type-cell-1", "Capital H, 1 of 16")
            expectLabel("large-type-cell-4", "Lowercase b, 4 of 16")
            expectLabel("large-type-cell-7", "Underscore, 7 of 16")
            snapshot("\(appearance)-large-type")
            // Once, and only when it can take the tap: a tap that lands
            // while the sheet is still coming up is not a press of Done.
            let done = app.buttons["large-type-done"]
            tapWhenHittable(done, "Done")
            expectGone(done, "Done should close the large-type view")

            tapWhenHittable(app.buttons["password-copy"], "Copy")
            snapshot("\(appearance)-copied")

            tapWhenHittable(app.buttons["password-reveal"], "The eye")
            expectValue("password-value", "hidden")

            tapWhenHittable(app.buttons["lock-now"], "Lock now")
            expectGone(element("face-id-window-text"), "Lock now should remove the Face ID window bar")
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
            XCTAssertTrue(waitToExist(app.staticTexts["No passwords yet"], timeout: Self.patience))
            XCTAssertTrue(app.staticTexts["No matches"].exists)
            snapshot(appearance)
            tapWhenHittable(app.buttons["empty-add"], "Add a password")
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
            expectLabel("face-id-github.com", contains: "github.com, web")
            expectLabel("face-id-github.com", contains: "opens with Face ID")
            expectLabel("recent-fastmail", "fastmail, email, needs your key")
            expectLabel("add-button", "Add a password")
            expect("settings-button")
            snapshot(appearance)

            let search = app.searchFields.firstMatch
            XCTAssertTrue(waitToExist(search, timeout: Self.patience), "Home should have a search field")
            focus(search)
            snapshot("\(appearance)-search-empty")
            search.typeText("ycombinator")
            expectLabel("search-count", "1 match")
            expectLabel("entry-news.ycombinator.com", "news.ycombinator.com, web, needs your key")
            expectGone(app.buttons["face-id-github.com"], "github.com does not match \"ycombinator\"")
            snapshot("\(appearance)-search")
        }
    }

    func testHomeFolders() {
        for appearance in Self.appearances {
            launch(page: "home", appearance: appearance)
            expect("home-title")
            app.swipeUp()
            expectLabel("folder-bank", "bank, folder, 3 passwords")
            expect("folder-web")
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
