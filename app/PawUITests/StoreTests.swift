import XCTest

/// A store set up through the simulated git host (`-pawSetup 1`), as it is
/// kept on the iPhone: protected at rest, and removed without leaving Face
/// ID copies, the PIN, keychain items or a password on the pasteboard.
final class StoreTests: PawUITestCase {
    private func launchSetup(keep: Bool = false) {
        app.launchArguments = ["-pawSetup", "1", "-pawAppearance", "light"] + keyArguments()
            + (keep ? ["-pawSetupKeep", "1"] : [])
        app.launch()
    }

    /// Welcome to Home: the simulated host, its key trusted, PIN once, two touches.
    private func setUpStore() {
        tap("setup-start")
        expect("setup-your-key")
        tap("key-continue")
        tap("simulated-address")
        tap("address-connect")
        tap("host-trust")
        tap("download-start")
        enterPin(Self.pin)
        touchTheKey()
        expect("setup-check", timeout: 60)
        tap("check-start")
        touchTheKey()
        expect("setup-done", timeout: 60)
        tap("done-open")
        expect("home-title")
    }

    /// Settings, scrolled to Remove, confirmed.
    private func removeStore() {
        openSettings()
        tap("settings-remove-store")
        expectText("settings-remove-warning", contains: "not touched")
        snapshot("remove-confirm")
        tap("settings-remove-confirm")
        expect("setup-welcome")
    }

    func testRemovingTheStoreLeavesNoCopiesPinOrKeychainItems() {
        launchSetup()
        expect("setup-welcome")
        setUpStore()
        openGitHubWithKey(askingForPin: false)
        app.buttons["copy-password"].tap()
        expectLabel("sim-last-toast", contains: "Password copied")
        goBack()
        goBack()
        expect("face-id-github.com")

        removeStore()
        let state = expect("sim-store-state")
        wait(for: "value CONTAINS 'copies 0 ' AND value CONTAINS 'pin none' AND value CONTAINS 'pasteboard empty'",
             on: state, timeout: Self.patience,
             "Nothing of the store should be left: \(String(describing: state.value))")
        wait(for: "value MATCHES '.*keychain resets [1-9].*'", on: state, timeout: Self.patience,
             "The Face ID keys should have been deleted: \(String(describing: state.value))")
        snapshot("removed")

        app.terminate()
        launchSetup(keep: true)
        expect("setup-welcome")
    }

    func testTheStoresFilesAreProtectedAtRest() {
        launchSetup()
        expect("setup-welcome")
        setUpStore()
        openSettings()
        showSimulatorControls()
        let protection = scrollTo("sim-file-protection")
        let summary = (protection.value as? String) ?? ""
        let attachment = XCTAttachment(string: summary)
        attachment.name = "file-protection"
        attachment.lifetime = .keepAlways
        add(attachment)
        XCTAssertFalse(summary.hasPrefix("items 0 "), "The store should have files: \(summary)")
        XCTAssertTrue(summary.contains("refused 0"), "Every item should take the complete class: \(summary)")
        XCTAssertTrue(summary.contains("other 0"), "No item should read back a weaker class: \(summary)")
        // The simulator's file system takes the class without recording it,
        // so the read-back is checked only where it is recorded.
        if !summary.hasSuffix("complete 0 · other 0 · unset \(itemCount(summary))") {
            XCTAssertTrue(summary.hasSuffix("unset 0"), "Every item should read back complete: \(summary)")
        }
    }

    private func itemCount(_ summary: String) -> String {
        summary.split(separator: " ").dropFirst().first.map(String.init) ?? ""
    }
}
