import XCTest

/// The Secure Enclave keystore and the system clocks, through the debug
/// check screen (`-pawEnclaveCheck 1`). The simulator has no Face ID to
/// match, so this proves the keychain plumbing and the clocks; what the
/// simulator says about the Secure Enclave itself is recorded, not
/// required. Face ID outcomes stay with `SimKeystore`.
final class EnclaveTests: PawUITestCase {
    override func setUp() {
        super.setUp()
        // Every row is recorded and checked, whatever an earlier one said.
        continueAfterFailure = true
        app.launchArguments = ["-pawEnclaveCheck", "1"]
        app.launch()
    }

    /// The row's text, after waiting for it (scrolling down if need be).
    private func text(_ identifier: String, file: StaticString = #filePath, line: UInt = #line) -> String {
        let found = scrollTo(identifier, file: file, line: line)
        return found.exists ? found.label : ""
    }

    private func record(_ name: String, _ value: String) {
        let attachment = XCTAttachment(string: value)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        print("EnclaveTests \(name): \(value)")
    }

    /// Runs the storage checks and waits for them to finish.
    private func runStorageChecks(file: StaticString = #filePath, line: UInt = #line) {
        tap("enclave-run-storage", file: file, line: line)
        // Every row appears at once when the checks finish.
        expect("enclave-probe", file: file, line: line)
    }

    /// The storage checks and the clocks, then the boot id across a
    /// relaunch: copies survive a relaunch until the phone restarts. One
    /// launch serves both; nothing stops at a failed row.
    func testStorageAndClocks() {
        record("faceid-state", text("enclave-faceid-state"))
        record("passcode-state", text("enclave-passcode-state"))
        let accessGroup = text("enclave-access-group")
        record("access-group", accessGroup)

        runStorageChecks()

        // Recorded: whether the simulator has a Secure Enclave and can make
        // a Face ID-bound key depends on the runtime and its Face ID setting.
        for row in ["enclave-probe", "enclave-create", "enclave-seal"] {
            let value = text(row)
            record(row, value)
            XCTAssertTrue(value.contains(": pass") || value.contains(": fail"), "\(row) has an outcome: \(value)")
        }

        // The keychain items. CI builds the simulator app unsigned, and an
        // app with no entitlements gets no keychain there at all
        // (errSecMissingEntitlement, -34018); then every keychain step
        // must say exactly that rather than pass or hang.
        let keychainMissing = accessGroup.contains("OSStatus -34018")
        for row in ["enclave-index-length", "enclave-index-same", "enclave-reset", "enclave-index-after-reset"] {
            let value = text(row)
            record(row, value)
            if keychainMissing {
                XCTAssertTrue(value.contains(": fail: OSStatus -34018"), "\(row): \(value)")
            } else {
                XCTAssertTrue(value.contains(": pass"), "\(row): \(value)")
            }
        }

        // The clocks: always required.
        for row in ["enclave-monotonic", "enclave-wall", "enclave-boot-id"] {
            let value = text(row)
            record(row, value)
            XCTAssertTrue(value.contains(": pass"), "\(row): \(value)")
        }
        XCTAssertEqual(text("enclave-done"), "Storage checks: done")

        // The boot id must outlive the process.
        let before = text("enclave-boot-shown")
        record("boot-before", before)
        XCTAssertTrue(before.hasPrefix("Boot id ") && !before.contains("unknown-"), before)
        app.terminate()
        app.launch()
        runStorageChecks()
        let after = text("enclave-boot-shown")
        record("boot-after", after)
        XCTAssertEqual(after, before, "the boot id should be the same after a relaunch")
    }

    func testFaceIdCheckAnswersWithoutHanging() {
        tap("enclave-check-faceid")
        // With no enrolled face the seal step refuses before any prompt.
        let seal = text("enclave-faceid-seal")
        record("faceid-seal", seal)
        XCTAssertTrue(seal.contains(": pass") || seal.contains(": fail"), seal)
        if seal.contains(": pass") {
            // An enrolled simulator face is asked; nobody answers here, so
            // only record whatever the unseal row says if it appears.
            let unseal = element("enclave-faceid-unseal")
            if waitToExist(unseal, timeout: 5) { record("faceid-unseal", unseal.label) }
        }
    }
}
