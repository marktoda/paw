import XCTest

/// Adding, editing, renaming and deleting on the fixture store
/// (`-pawFixture 1`), which is set up through the simulated git host so it
/// can be written, and the list of changes not sent yet. None of it needs
/// the key: the key sheet only appears to open an entry.
final class EditTests: PawUITestCase {
    // MARK: Helpers

    private func launch(appearance: String = "light", extra: [String] = []) {
        launchFixture(appearance: appearance, extra: extra)
        expectAbsent("warning-row", "The fixture store should be writable")
    }

    /// Save is dimmed and does nothing: the sheet stays and nothing is saved.
    private func expectSaveBlocked(file: StaticString = #filePath, line: UInt = #line) {
        app.buttons["form-save"].firstMatch.tap()
        XCTAssertTrue(element("form-cancel").exists, "Save should not close the form", file: file, line: line)
        XCTAssertFalse(element("sim-last-app-toast").exists, "Nothing should have been saved", file: file, line: line)
    }

    /// Types into a form field, replacing what is there.
    private func type(_ text: String, into identifier: String, file: StaticString = #filePath, line: UInt = #line) {
        let field = scrollTo(identifier, file: file, line: line)
        focus(field, file: file, line: line)
        let current = (field.value as? String) ?? ""
        let placeholder = field.placeholderValue ?? ""
        if !current.isEmpty && current != placeholder {
            field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: current.count))
        }
        if !text.isEmpty {
            // The form can redraw and take the keyboard away between the tap
            // and the typing; typing then fails outright, so take it back.
            if !hasFocus(field) { focus(field, file: file, line: line) }
            field.typeText(text)
        }
    }

    private func clear(_ identifier: String) {
        type("", into: identifier)
    }

    private func chooseFolder(_ folder: String) {
        // The first tap on the menu can be spent on putting the keyboard away.
        let item = app.buttons[folder].firstMatch
        for _ in 0..<3 where !item.exists {
            tap("form-folder")
            if waitUntil(timeout: 5, { item.exists }) { break }
        }
        XCTAssertTrue(item.exists, "The folder menu should list \(folder)")
        item.tap()
        expectValue("form-folder", folder)
    }

    private func expectNoKeySheet(file: StaticString = #filePath, line: UInt = #line) {
        expectAbsent("key-sheet-purpose", "Saving should never ask for the key", file: file, line: line)
        expectAbsent("pin-field", "Saving should never ask for the PIN", file: file, line: line)
    }

    /// From Home or a folder: a new entry with a typed password, saved.
    private func addEntry(_ name: String, folder: String, username: String = "", password: String = "Typed-pass-42") {
        tap("add-button")
        expect("form-name")
        type(name, into: "form-name")
        if !folder.isEmpty {
            chooseFolder(folder)
        }
        if !username.isEmpty {
            type(username, into: "form-username")
        }
        type(password, into: "form-password")
        expectEnabled("form-save", true)
        tap("form-save")
        expectGone("form-save")
        expectNoKeySheet()
    }

    /// Opens `name` in the folder on screen with the PIN (first time) and a touch.
    private func openWithKey(_ name: String, askingForPin: Bool = true) {
        tap("entry-\(name)")
        expectLabel("key-sheet-purpose", contains: "Open \(name)")
        if askingForPin {
            enterPin(Self.pin)
        }
        touchTheKey()
        expectLabel("entry-title", contains: name)
    }

    // MARK: Add

    func testAddingWithAGeneratedPasswordSavesWithoutAKeyPromptAndAppearsInItsFolderAndSearch() {
        launch()
        tap("add-button")
        expect("form-name")
        snapshot("add-empty-light")
        type("shop.example", into: "form-name")
        chooseFolder("web")
        type("shopper", into: "form-username")
        tap("form-generate")
        let password = expect("form-password")
        XCTAssertEqual((password.value as? String)?.count, 20, "Generate should fill a 20-character password")
        type("https://shop.example/login", into: "form-website")
        snapshot("add-filled-light")
        scrollTo("form-recipients")
        expectLabel("form-recipients", contains: "Encrypted for your key")

        tap("form-save")
        expectGone("form-save")
        expectNoKeySheet()
        expectLabel("sim-last-app-toast", contains: "Saved. Not sent yet.")
        expectLabel("unpushed-banner", contains: "1 change not sent")
        snapshot("home-after-save")

        // Saved with a Face ID copy: what was just typed opens without the key.
        tap("folder-web")
        expectLabel("entry-shop.example", contains: "shop.example, opens with Face ID")
        expectNoKeySheet()
        goBack()

        let search = app.searchFields.firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: 10))
        search.tap()
        search.typeText("shop")
        expect("entry-shop.example")
    }

    func testANewEntryOpensWithFaceIdAndShowsWhatWasTyped() {
        launch()
        tap("folder-email")
        tap("add-button")
        expectValue("form-folder", "email")
        type("newsletter", into: "form-name")
        type("reader@example.com", into: "form-username")
        type("Quiet-Harbour-7", into: "form-password")
        type("https://news.example", into: "form-website")
        type("Weekly, on Fridays.", into: "form-notes")
        tap("form-add-field")
        type("member id", into: "form-field-key-0")
        type("A-1234", into: "form-field-value-0")
        tap("form-save")
        expectGone("form-save")
        expectNoKeySheet()

        // A new entry gets a Face ID copy from its save: no key sheet.
        expectLabel("entry-newsletter", contains: "opens with Face ID")
        tap("entry-newsletter")
        expectLabel("entry-title", contains: "newsletter")
        expectAbsent("key-sheet-purpose", "What was just typed opens with Face ID, not the key")
        expectLabel("face-id-window-text", contains: "Opens with Face ID until")
        expectValue("field-Username", "reader@example.com")
        expectValue("field-Website", "https://news.example")
        expectValue("field-Member id", "A-1234")
        expectLabel("entry-notes", contains: "Weekly, on Fridays.")
        tap("password-reveal")
        expectValue("password-value", "Quiet-Harbour-7")
        snapshot("new-entry-opened")
    }

    func testAnEmptyADuplicateAndASlashedNameEachShowTheCoresMessageAndBlockSave() {
        for appearance in ["light", "dark"] {
            launch(appearance: appearance)
            tap("add-button")
            expect("form-name")
            if appearance == "dark" {
                snapshot("add-empty-dark")
            }
            expectSaveBlocked()

            type("x", into: "form-name")
            clear("form-name")
            expectLabel("problem-name", contains: "Give it a name.")
            expectSaveBlocked()

            type("work/vpn", into: "form-name")
            expectLabel("problem-name", contains: "Names can't contain /.")
            expectSaveBlocked()

            chooseFolder("web")
            type("GitHub.com", into: "form-name")
            expectLabel("problem-name", contains: "There's already an entry called 'github.com' in web.")
            expectSaveBlocked()
            snapshot("add-errors-\(appearance)")

            type("github.com-2", into: "form-name")
            expectGone("problem-name")
            expectEnabled("form-save", true)
            app.terminate()
        }
    }

    func testGenerateFillsAPasswordOfTheChosenLengthAndTheStrengthFollowsTheLength() {
        launch()
        tap("add-button")
        expect("form-name")
        tap("form-options")
        let slider = app.sliders["form-length"]
        XCTAssertTrue(slider.waitForExistence(timeout: 10), "The options should show the length")
        expectLabel("form-strength", contains: "very strong")

        scrollTo("form-length")
        slider.adjust(toNormalizedSliderPosition: 0)
        // A drag that starts while the form is still settling moves nothing.
        if (slider.value as? String) != "8 characters" {
            slider.adjust(toNormalizedSliderPosition: 0)
        }
        expectValue("form-length", "8 characters")
        expectLabel("form-strength", contains: "fair")
        tap("form-generate")
        XCTAssertEqual((expect("form-password").value as? String)?.count, 8)
        snapshot("generator-options")

        // A synthesized drag to the end can stop a step or two short, and
        // stops far short when it starts while the list is still moving
        // after Generate (once it left the slider at 28): bring the slider
        // to rest on screen first, then read the length it settled on
        // rather than assuming 64. A drag cut short is dragged again.
        let long = NSPredicate(format: "value MATCHES %@", "^(4[89]|5[0-9]|6[0-4]) characters$")
        for _ in 0..<3 where !long.evaluate(with: slider) {
            scrollTo("form-length")
            slider.adjust(toNormalizedSliderPosition: 1)
            _ = XCTWaiter().wait(for: [XCTNSPredicateExpectation(predicate: long, object: slider)], timeout: 5)
        }
        XCTAssertTrue(long.evaluate(with: slider),
                      "The slider should move to a long length, it shows \(slider.value ?? "nothing")")
        expectLabel("form-strength", contains: "very strong")
        // Compare live values: the slider may still be settling as this runs.
        let password = element("form-password")
        let regenerated = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            let length = Int(((slider.value as? String) ?? "").prefix { $0.isNumber }) ?? 0
            return length >= 48 && (password.value as? String)?.count == length
        }, object: nil)
        XCTAssertEqual(XCTWaiter().wait(for: [regenerated], timeout: 15), .completed,
                       "A generated password should follow the new length")
    }

    func testCancelWithSomethingTypedAsksInlineBeforeDiscarding() {
        launch()
        tap("add-button")
        type("half-typed", into: "form-name")
        tap("form-cancel")
        expectLabel("form-discard-question", contains: "Discard this new password?")
        snapshot("cancel-confirm")
        tap("form-discard-cancel")
        expectGone("form-discard-question")
        expectValue("form-name", "half-typed")

        tap("form-cancel")
        tap("form-discard-confirm")
        expectGone("form-name")
        expectAbsent("unpushed-banner", "Nothing should have been saved")
    }

    // MARK: Edit, rename, delete

    func testEditingTheUsernameOfAnOpenEntryShowsItAndKeepsThePassword() {
        launch()
        tap("folder-web")
        openWithKey("github.com")
        tap("entry-edit")
        expectValue("form-username", "you@example.com")
        type("new-you@example.com", into: "form-username")
        snapshot("edit-form")
        tap("form-save")
        expectGone("form-save")
        expectNoKeySheet()

        expectValue("field-Username", "new-you@example.com")
        tap("password-reveal")
        expectValue("password-value", Self.githubPassword)
        expect("field-one-time-code")
        snapshot("edited-entry")
    }

    func testRenamingAndMovingAnEntryMovesItInTheList() {
        launch()
        tap("folder-web")
        openWithKey("gitlab.com")
        tap("entry-more")
        tap("entry-rename")
        type("gitlab-old", into: "form-name")
        chooseFolder("email")
        tap("form-save")
        expectGone("form-save")
        expectNoKeySheet()
        expectLabel("entry-title", contains: "gitlab-old")
        expectLabel("entry-folder", contains: "email")

        goBack()
        expectGone("entry-gitlab.com")
        goBack()
        tap("folder-email")
        expect("entry-gitlab-old")
    }

    func testDeletingAnEntryRemovesItAndUndoBringsItBack() {
        launch()
        tap("folder-web")
        openWithKey("example.com")
        tap("entry-more")
        tap("entry-delete")
        expectLabel("entry-delete-question", contains: "Delete example.com?")
        expectLabel("entry-delete-question", contains: "It stays in the store's history.")
        snapshot("delete-confirm")
        tap("entry-delete-confirm")

        expect("entry-github.com")
        expectGone("entry-example.com")
        expectLabel("action-toast-message", contains: "Deleted example.com.")
        app.buttons["action-toast-button"].tap()
        expect("entry-example.com")
        expectLabel("sim-last-app-toast", contains: "example.com is back.")
    }

    // MARK: Not sent yet

    func testTheBannerCountsChangesAndTheListShowsWhatEachDid() {
        launch()
        addEntry("first.example", folder: "web")
        expectLabel("unpushed-banner", contains: "1 change not sent")
        addEntry("second.example", folder: "")
        expectLabel("unpushed-banner", contains: "2 changes not sent")
        tap("unpushed-banner")
        expectLabel("unpushed-count", contains: "2 changes")
        expectLabel("unpushed-added-first.example", contains: "Added first.example")
        expectLabel("unpushed-added-second.example", contains: "Added second.example")
        expectLabel("unpushed-send-footer", contains: "Needs your key")
        snapshot("unpushed-list")
    }

    func testDiscardingOneChangeTakesBackOnlyThatChange() {
        launch()
        addEntry("keep.example", folder: "web")
        addEntry("drop.example", folder: "web")
        tap("unpushed-banner")
        tap("unpushed-discard-button-keep.example")
        expectLabel("unpushed-discard-keep.example-question", contains: "Discard \u{201C}Added keep.example\u{201D}?")
        snapshot("discard-confirm")
        tap("unpushed-discard-keep.example-confirm")
        expectGone("unpushed-added-keep.example")
        expect("unpushed-added-drop.example")

        goBack()
        expectLabel("unpushed-banner", contains: "1 change not sent")
        tap("folder-web")
        expect("entry-drop.example")
        expectAbsent("entry-keep.example", "A discarded add should take the entry away")
        goBack()

        tap("unpushed-banner")
        tap("unpushed-discard-all")
        expectLabel("unpushed-discard-all-question", contains: "Discard 1 change?")
        tap("unpushed-discard-all-confirm")
        expectGone("unpushed-added-drop.example")
        goBack()
        expectGone("unpushed-banner")
    }

    // MARK: Who can open a folder

    func testChangedRecipientsAskForAReviewBeforeSavingThere() {
        launch(extra: ["-pawSimRecipients", "changed"])
        tap("folder-work")
        tap("add-button")
        expectValue("form-folder", "work")
        type("intranet", into: "form-name")
        type("Typed-pass-42", into: "form-password")
        scrollTo("warning-row")
        expectLabel("warning-row", contains: "Who can open work changed")
        expectSaveBlocked()
        snapshot("recipients-changed")
        scrollTo("error-action").tap()
        expect("recipients-accept")
        snapshot("recipients-review")
        tap("recipients-accept")
        scrollTo("form-recipients")
        expectLabel("form-recipients", contains: "Encrypted for your key")
        expectEnabled("form-save", true)
        tap("form-save")
        expectGone("form-save")
        expectNoKeySheet()
        expect("entry-intranet")
    }

    func testAFolderWithAKeyPawLacksRefusesToSaveForFewerKeys() {
        launch(extra: ["-pawSimRecipients", "missing"])
        tap("folder-work")
        tap("add-button")
        type("intranet", into: "form-name")
        tap("error-action")
        tap("recipients-accept")
        scrollTo("warning-row")
        expectLabel("warning-row", contains: "also encrypted for a key paw doesn't have")
        expectSaveBlocked()
        snapshot("recipients-missing")
    }

    // MARK: Larger text

    func testTheAddFormKeepsItsLabelsAtALargerTextSize() {
        launch(extra: ["-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityM"])
        tap("add-button")
        expect("form-name")
        snapshot("add-large-text")
        tap("form-options")
        scrollTo("form-strength")
        snapshot("add-large-text-options")
    }
}
