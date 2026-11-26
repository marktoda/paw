import XCTest

/// The background check with the GitHub sign-in. iOS never runs a
/// `BGAppRefreshTask` in the simulator, so `-pawSimBackgroundRefresh 1`
/// runs the task's body each time paw goes to the background. Against the
/// core's simulated GitHub (`-pawSimGitHub`); nothing talks to the real one.
final class BackgroundRefreshTests: PawUITestCase {
    private static let store = "github-repo-mira/password-store"

    /// Setup through "Choose from GitHub", to Home. A change waits on the
    /// server (`-pawSimRemoteChange add`: `web/forum.example`).
    private func setUpThroughGitHub() {
        app.launchArguments = [
            "-pawSetup", "1", "-pawSimGitHub", "signedOut", "-pawSimRemoteChange", "add",
            "-pawSimBackgroundRefresh", "1",
        ] + keyArguments()
        app.launch()
        expect("setup-welcome")
        tap("setup-start")
        expect("setup-your-key")
        tapUntilGone("key-continue")
        expect("repo-address")
        tap("address-github")
        expectText("github-code", contains: "WDJB-MJHT")
        tap("sim-github-authorise")
        expect("github-repositories", timeout: Self.patience)
        tap(Self.store)
        expect("setup-confirm-host")
        tap("host-trust")
        expect("setup-download")
        tap("download-start")
        enterPin(Self.pin)
        touchTheKey()
        expect("setup-check", timeout: 60)
        tap("check-start")
        touchTheKey()
        expect("setup-done")
        tap("done-open")
        expect("home-title")
    }

    func testABackgroundCheckBringsInARemoteChangeWithoutTheKey() {
        setUpThroughGitHub()
        expectText("folder-web", contains: "web, folder, 6 passwords")

        backgroundAndReturn()

        expectText("folder-web", contains: "web, folder, 7 passwords")
        waitForKeyTasksToEnd()
        expectAbsent("key-sheet-purpose", "A background check never asks for the key")
        expectAbsent("pin-field", "A background check never asks for the PIN")
        tap("folder-web")
        expect("entry-forum.example")
        goBack()
        tap("sync-pill")
        expect("sync-sheet")
        expectLabel("sync-background", contains: "Brought in changes")
        expectLabel("sync-last-checked", contains: "Just now")
        snapshot("sync-sheet-background")
    }
}
