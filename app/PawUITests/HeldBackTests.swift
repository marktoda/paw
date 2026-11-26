import XCTest

/// Re-encrypting a held-back change on the fixture store: another computer
/// gives `work/` a second key paw has the public half of
/// (`-pawSimRemoteChange secondKey`), a change saved in `work` is held
/// back by the send that pulls it, and once the new list is accepted
/// "Re-encrypt 1 change" opens it with the key, a touch, and encrypts it
/// for both.
final class HeldBackTests: PawUITestCase {
    // MARK: Helpers

    /// Types into a form field, replacing nothing: the form is new.
    private func type(_ text: String, into identifier: String) {
        let field = scrollTo(identifier)
        focus(field)
        // The form can redraw and take the keyboard away between the tap
        // and the typing.
        if !hasFocus(field) { focus(field) }
        field.typeText(text)
    }

    private func chooseFolder(_ folder: String) {
        // The first tap on the menu can be spent on putting the keyboard away.
        let item = app.buttons[folder].firstMatch
        for _ in 0..<3 where !item.exists {
            tap("form-folder")
            if waitUntil(timeout: 5, { item.exists }) { break }
        }
        tapWhenHittable(item, "The folder menu's \(folder)")
        expectValue("form-folder", folder)
    }

    /// Waits for the key sheet to ask for the PIN, a touch, or to finish,
    /// and gives it what it asks for until it has finished.
    private func passKeySheet(purpose: String, file: StaticString = #filePath, line: UInt = #line) {
        expectLabel("key-sheet-purpose", contains: purpose, file: file, line: line)
        let touch = element("touch-prompt")
        let purposeLabel = element("key-sheet-purpose")
        let field = element("pin-field")
        for _ in 0..<3 {
            var asked = "nothing"
            let settled = waitUntil(timeout: 45) {
                if touch.exists {
                    asked = "touch"
                } else if !purposeLabel.exists {
                    asked = "finished"
                } else if field.exists {
                    asked = "pin"
                }
                return asked != "nothing"
            }
            guard settled else {
                return XCTFail("The key sheet neither asked for anything nor finished", file: file, line: line)
            }
            switch asked {
            case "touch":
                tapSimulated("simulated-touch", file: file, line: line)
                expectGone("touch-prompt", file: file, line: line)
            case "pin":
                enterPin(Self.pin, file: file, line: line)
                expectGone("pin-field", file: file, line: line)
            default:
                return
            }
        }
    }

    /// A change saved in `work`, then a send that pulls the second key's
    /// list and holds it back; the held-back screen with the new list
    /// accepted, offering "Re-encrypt 1 change".
    private func holdBackAChangeAndAcceptTheNewList() {
        launchFixture(extra: ["-pawSimRemoteChange", "secondKey"])
        tap("add-button")
        expect("form-name")
        type("held.example", into: "form-name")
        chooseFolder("work")
        type("Typed-pass-42", into: "form-password")
        expectEnabled("form-save", true)
        tap("form-save")
        expectGone("form-save")
        expectLabel("sim-last-app-toast", contains: "Saved. Not sent yet.")
        tap("sync-pill")
        expect("sync-sheet-done")
        tap("sync-send")
        passKeySheet(purpose: "Send 1 change")
        let row = expect("sync-held-back")
        expectAbsent("sync-send", "A held-back change is never offered to send as it is")
        tapWhenHittable(row.buttons["error-action"].firstMatch, "The held-back row's Review")
        expect("held-back-held.example")
        expectLabel("held-back-should-be-for-held.example", contains: "Not accepted on this iPhone yet.")
        expectAbsent("held-back-reencrypt", "Nothing can be re-encrypted before the new list is accepted")
        tap("held-back-review-held.example")
        tap("recipients-accept")
        expectLabel("held-back-reencrypt", contains: "Re-encrypt 1 change")
    }

    // MARK: Tests

    func testReencryptingAHeldBackChangeLetsItBeSentAndCancelLeavesItHeldBack() {
        holdBackAChangeAndAcceptTheNewList()
        snapshot("reencrypt-offered")

        tap("held-back-reencrypt")
        expectLabel("key-sheet-purpose", contains: "Re-encrypt 1 change")
        // The PIN is still held from the send: the touch comes next.
        expect("touch-prompt")
        tap("key-sheet-cancel")
        expectGone("key-sheet-purpose")
        expect("held-back-held.example")
        expectLabel("held-back-reencrypt", contains: "Re-encrypt 1 change")
        expectEnabled("held-back-reencrypt", true)

        tap("held-back-reencrypt")
        passKeySheet(purpose: "Re-encrypt 1 change")
        // The sync sheet stays up, now offering to send it as it is.
        expectLabel("sync-send", contains: "Send 1 change")
        expectGone("sync-held-back")
        snapshot("reencrypted")
        tap("sync-sheet-done")
        expectGone("sync-sheet-done")
        expectLabel("sim-last-app-toast", contains: "Re-encrypted. Not sent yet.")
        expect("held-back-empty")
    }
}
