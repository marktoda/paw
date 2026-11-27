import XCTest

/// The review fixes of 2026-11-24: a launch while the phone is locked, a
/// server key that changed after Forget, and the rest of that pass.
final class PolishThreeTests: PawUITestCase {
    // MARK: Launch and iPad

    /// iOS may launch paw in the background while the phone is locked, when
    /// the store can't be read. That is not a broken store: paw shows
    /// nothing, offers no delete, and opens Home once the phone is unlocked.
    func testALockedLaunchWaitsForTheUnlock() {
        let setup = ["-pawSetup", "1", "-pawAppearance", "light"] + keyArguments()
        app.launchArguments = setup
        app.launch()
        tap("setup-start")
        expect("setup-your-key")
        tapUntilGone("key-continue")
        tap("simulated-address", on: "repo-address")
        tap("address-connect")
        tap("host-trust", on: "setup-confirm-host")
        tap("download-start", on: "setup-download")
        enterPin(Self.pin)
        touchTheKey()
        expect("setup-check", timeout: 60)
        tap("check-start")
        touchTheKey()
        expect("setup-done", timeout: 60)
        tap("done-open")
        expect("home-title")

        app.terminate()
        app.launchArguments = setup + ["-pawSetupKeep", "1", "-pawSimLockedLaunch", "1"]
        app.launch()
        expect("store-waiting-for-unlock")
        expectAbsent("store-problem", "A locked phone is not a broken store")
        expectAbsent("store-start-over", "A locked phone must never offer to delete the store")
        snapshot("locked-launch")

        tapSimulated("sim-unlock")
        expect("home-title")
    }

    // MARK: Settings

    /// The server's keys in Settings; Forget, then Check, when the server
    /// now presents another key: paw says it differs from the one
    /// forgotten, and trusting it takes a second, deliberate step.
    func testAServerKeyThatChangedAfterForgetIsFlaggedAndTrustedOnlyOnConfirm() {
        launchFixture(extra: ["-pawSimRemoteChange", "changedHostKey"])
        openSettings()
        // The simulated host is not one whose keys ship with paw.
        expectText("settings-host-fingerprint", contains: "Trusted by you")
        tap("settings-host-fingerprint")
        scrollTo("settings-host-fingerprints")
        expectLabel("settings-host-key", contains: "Ed25519")
        expectLabel("settings-host-key", contains: "SHA256:")
        snapshot("server-keys")

        tap("settings-forget-host-key")
        tap("settings-forget-host-confirm")
        expectText("settings-host-fingerprint", contains: "Not trusted yet")
        tap("settings-check-host")
        scrollTo("settings-presented-key-differs")
        expectLabel("settings-presented-key-differs", contains: "Different from the key you trusted before")
        expectAbsent("settings-presented-key-same", "A different key is never called the same")
        snapshot("server-key-differs")

        // Trust asks again before it trusts.
        tap("settings-trust-host")
        expect("settings-trust-different-question")
        expectText("settings-host-fingerprint", contains: "Not trusted yet")
        tap("settings-trust-different-confirm")
        expectText("settings-host-fingerprint", contains: "Trusted by you")
    }

    // MARK: AutoFill
}
