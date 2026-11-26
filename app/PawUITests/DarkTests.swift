import XCTest

/// The main screens in dark, for their pictures: setup's first screens,
/// Home, the key sheet, an entry, the sync sheet and the AutoFill list.
/// Nothing compares the pictures; they are looked at in the CI report.
/// What each screen does is tested in light, by the other classes.
final class DarkTests: PawUITestCase {
    func testTheMainScreensInDark() {
        app.launchArguments = ["-pawSetup", "1", "-pawAppearance", "dark"] + keyArguments()
        app.launch()
        expect("setup-welcome")
        snapshot("setup-welcome")
        tap("setup-start")
        expect("setup-your-key")
        snapshot("setup-your-key")
        tapUntilGone("key-continue")
        expect("repo-address")
        snapshot("setup-address")
        app.terminate()

        launchFixture(appearance: "dark")
        snapshot("home")
        tap("folder-web")
        tap("entry-github.com")
        pinField()
        snapshot("key-sheet-pin")
        enterPin(Self.pin)
        expect("touch-prompt")
        snapshot("key-sheet-touch")
        tapSimulated("simulated-touch")
        expectLabel("entry-title", contains: "github.com")
        expectGone("key-sheet-purpose")
        snapshot("entry")
        goBack()
        goBack()

        // The pill checks with the key first (the PIN is held); once up to
        // date it opens the sync sheet.
        expectLabel("sync-pill", contains: "Updates need your key")
        tap("sync-pill")
        // A touch is asked for unless the key's is still cached.
        let purpose = expect("key-sheet-purpose")
        let touch = element("touch-prompt")
        _ = waitUntil(timeout: Self.patience) { touch.exists || !purpose.exists }
        if touch.exists { tapSimulated("simulated-touch") }
        expectGone("key-sheet-purpose")
        expectLabel("sync-pill", contains: "Up to date")
        tap("sync-pill")
        expect("sync-sheet-done")
        snapshot("sync-sheet")
        app.terminate()

        app.launchArguments = ["-pawAppearance", "dark", "-pawHostProvider", "https://github.com"] + keyArguments()
        app.launch()
        expect("autofill-entry-web/github.com")
        snapshot("autofill-list")
    }
}
