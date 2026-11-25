import XCTest

/// One-time codes on the fixture store: the code made from an entry's
/// `otpauth://` line, copying it, and adding one in the editor. The codes
/// themselves are checked against RFC 6238 in the core (core/tests/otp.rs).
final class OneTimeCodeTests: PawUITestCase {
    /// "123 456" as shown, once it is six digits.
    private func shownCode(file: StaticString = #filePath, line: UInt = #line) -> String {
        let row = expect("field-one-time-code", file: file, line: line)
        var shown = ""
        let isCode = waitUntil(timeout: Self.patience) {
            shown = (row.value as? String) ?? ""
            return shown.range(of: #"^\d{3} \d{3}$"#, options: .regularExpression) != nil
        }
        XCTAssertTrue(isCode, "The row should show a six-digit code, it showed \"\(shown)\"", file: file, line: line)
        return shown
    }

    func testTheFixtureEntryShowsACodeThatCopies() {
        launchFixture()
        openGitHubWithKey()
        let shown = scrollTo("field-one-time-code")
        XCTAssertTrue(shown.exists)
        let code = shownCode()
        snapshot("one-time-code")

        tap("copy-one-time-code")
        expectLabel("sim-last-toast", contains: "One-time code copied")
        // The simulated clock stands still, so the code copied is the one
        // shown. Settings, where the pasteboard is read, opens from Home.
        goBack()
        goBack()
        expect("home-title")
        XCTAssertEqual(pasteboardContents(), code.replacingOccurrences(of: " ", with: ""))
    }

    func testAddingASetupKeyInTheEditorShowsTheCode() {
        launchFixture()
        tap("folder-web")
        tap("entry-gitlab.com")
        expectLabel("key-sheet-purpose", contains: "Open gitlab.com")
        enterPin(Self.pin)
        touchTheKey()
        expectLabel("entry-title", contains: "gitlab.com")
        expectAbsent("field-one-time-code", "gitlab.com has no one-time code yet")

        tap("entry-edit")
        let field = scrollTo("form-setup-key")
        focus(field)
        field.typeText("not a key!")
        expect("problem-setup-key")
        field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 10))
        if !hasFocus(field) { focus(field) }
        field.typeText("jbsw y3dp ehpk 3pxp")
        expectGone("problem-setup-key")
        tap("form-save")
        expectGone("form-save")

        _ = shownCode()
    }
}
