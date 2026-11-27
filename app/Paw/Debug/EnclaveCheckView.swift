#if DEBUG
import CryptoKit
import LocalAuthentication
import SwiftUI

/// `-pawEnclaveCheck 1`: a debug screen that exercises the Secure Enclave
/// keystore and the system clocks, for the UI test in the simulator and
/// for the owner on a phone. Results show pass or fail, sizes, equality
/// and raw error codes; never a key or a sealed value's contents.
///
/// It uses its own keychain namespace, so it never touches the keys of
/// real Face ID copies.
enum EnclaveCheckLaunch {
    static var isRequested: Bool {
        LaunchArgument.enclaveCheck.bool
    }
}

struct EnclaveCheckView: View {
    private struct Row: Identifiable, Sendable {
        let id: String
        let text: String
    }

    @State private var status: [Row] = []
    @State private var storage: [Row] = []
    @State private var faceId: [Row] = []
    @State private var busy = false
    /// The last Face ID test: what was sealed, and a digest of the random
    /// value to compare with (the value itself is not kept).
    @State private var lastSealed: Data?
    @State private var lastDigest: Data?

    nonisolated private static let namespace = "com.marktoda.paw.copies-check"

    var body: some View {
        NavigationStack {
            List {
                Section("This device") {
                    ForEach(status) { row in line(row) }
                }
                Section {
                    Button("Run storage checks") { runStorageChecks() }
                        .accessibilityIdentifier("enclave-run-storage")
                        .disabled(busy)
                    ForEach(storage) { row in line(row) }
                } header: {
                    Text("Without Face ID")
                } footer: {
                    Text("Seals with the public key, reads the index key twice, resets, and reads the clocks.")
                }
                Section {
                    Button("Check Face ID storage") { runFaceIdCheck(again: false) }
                        .accessibilityIdentifier("enclave-check-faceid")
                        .disabled(busy)
                    Button("Open it again with Face ID") { runFaceIdCheck(again: true) }
                        .accessibilityIdentifier("enclave-check-faceid-again")
                        .disabled(busy || lastSealed == nil)
                    ForEach(faceId) { row in line(row) }
                } header: {
                    Text("With Face ID")
                } footer: {
                    Text("Face ID should be asked every time, with no passcode button.")
                }
            }
            .navigationTitle("Face ID storage")
        }
        .onAppear { status = Self.deviceStatus() }
    }

    private func line(_ row: Row) -> some View {
        Text(row.text)
            .font(.footnote.monospaced())
            .accessibilityIdentifier(row.id)
    }

    private static func deviceStatus() -> [Row] {
        let biometry = LAContext()
        var error: NSError?
        let faceIdText: String
        if biometry.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error) {
            faceIdText = biometry.biometryType == .faceID ? "enrolled" : "enrolled (type \(biometry.biometryType.rawValue))"
        } else {
            faceIdText = "not usable: \(laText(error))"
        }
        biometry.invalidate()

        let owner = LAContext()
        var ownerError: NSError?
        let passcodeText: String
        if owner.canEvaluatePolicy(.deviceOwnerAuthentication, error: &ownerError) {
            passcodeText = "set"
        } else if ownerError?.code == LAError.Code.passcodeNotSet.rawValue {
            passcodeText = "not set"
        } else {
            passcodeText = "unknown: \(laText(ownerError))"
        }
        owner.invalidate()

        let group: String
        do { group = try KeychainGroup.shared() } catch let failure as EnclaveFailure {
            group = "fail: \(failure.detail)"
        } catch { group = "fail" }

        return [
            Row(id: "enclave-faceid-state", text: "Face ID: \(faceIdText)"),
            Row(id: "enclave-passcode-state", text: "Passcode: \(passcodeText)"),
            Row(id: "enclave-access-group", text: "Keychain group: \(group)"),
        ]
    }

    private static func laText(_ error: NSError?) -> String {
        guard let error else { return "no error given" }
        let names: [Int: String] = [
            LAError.Code.biometryNotEnrolled.rawValue: "not enrolled",
            LAError.Code.biometryNotAvailable.rawValue: "not available",
            LAError.Code.biometryLockout.rawValue: "locked out",
            LAError.Code.passcodeNotSet.rawValue: "passcode not set",
        ]
        return "\(names[error.code] ?? error.domain) (\(error.code))"
    }

    private func runStorageChecks() {
        busy = true
        storage = []
        DispatchQueue.global(qos: .userInitiated).async {
            let rows = Self.storageChecks()
            DispatchQueue.main.async {
                storage = rows
                busy = false
            }
        }
    }

    nonisolated private static func storageChecks() -> [Row] {
        var rows: [Row] = []
        func add(_ id: String, _ text: String) { rows.append(Row(id: "enclave-\(id)", text: text)) }
        let keystore = EnclaveKeystore(namespace: namespace)

        add("probe", "Secure Enclave round trip (no Face ID): \(EnclaveProbe.roundTrip())")
        add("create", "Seal key: " + outcome { "pass (\(try keystore.prepare()))" })

        var secret = EnclaveProbe.randomBytes()
        add("seal", "Seal 32 bytes: " + outcome {
            let sealed = try keystore.sealDetailed(Data(secret))
            return "pass (\(sealed.count) bytes)"
        })
        wipe(&secret)

        var first = Data()
        var second = Data()
        add("index-length", "Index key: " + outcome {
            first = try keystore.indexKeyDetailed()
            return first.count == 32 ? "pass (32 bytes)" : "fail: \(first.count) bytes"
        })
        add("index-same", "Index key read twice: " + outcome {
            second = try keystore.indexKeyDetailed()
            return second == first && !first.isEmpty ? "pass (same)" : "fail: different"
        })
        add("reset", "Reset: " + outcome {
            try keystore.deleteCopyKeys()
            return "pass"
        })
        add("index-after-reset", "Index key after reset: " + outcome {
            var after = try keystore.indexKeyDetailed()
            defer { wipe(&after) }
            return after.count == 32 && after != first ? "pass (different)" : "fail: same as before reset"
        })
        wipe(&first)
        wipe(&second)
        // Leave nothing of the check's own behind.
        _ = try? keystore.deleteCopyKeys()

        let clock = SystemClock()
        var previous = clock.monotonicMs()
        let start = previous
        var wentBack = false
        for _ in 0..<10_000 {
            let now = clock.monotonicMs()
            if now < previous { wentBack = true }
            previous = now
        }
        usleep(200_000)
        let slept = clock.monotonicMs() - start
        add("monotonic", wentBack || slept < 150
            ? "Monotonic clock: fail (went back: \(wentBack), 200 ms sleep read \(slept) ms)"
            : "Monotonic clock: pass (never went back; 200 ms sleep read \(slept) ms)")

        let wall = clock.wallMs()
        add("wall", wall > 1_700_000_000_000 ? "Wall clock: pass" : "Wall clock: fail (\(wall))")

        let boot = BootSession.read()
        let bootAgain = clock.bootId()
        add("boot-id", "Boot id read twice: \(boot.id == bootAgain ? "pass (same)" : "fail (changed)")")
        // The start of the id (not a secret) and its source, so a relaunch
        // can be compared: it must stay the same until the phone restarts.
        add("boot-shown", "Boot id \(boot.id.prefix(24))… from \(boot.source)")
        add("done", "Storage checks: done")
        return rows
    }

    private func runFaceIdCheck(again: Bool) {
        busy = true
        faceId = []
        let sealedBefore = lastSealed
        let digestBefore = lastDigest
        DispatchQueue.global(qos: .userInitiated).async {
            var rows: [Row] = []
            func add(_ id: String, _ text: String) { rows.append(Row(id: "enclave-faceid-\(id)", text: text)) }
            let keystore = EnclaveKeystore(namespace: Self.namespace)
            var sealed = sealedBefore
            var digest = digestBefore

            if !again {
                var secret = EnclaveProbe.randomBytes()
                digest = Data(SHA256.hash(data: secret))
                add("seal", "1. Seal a random value: " + outcome {
                    sealed = try keystore.sealDetailed(Data(secret))
                    return "pass"
                })
                wipe(&secret)
            }

            if let blob = sealed, let digest {
                var opened: Data?
                add("unseal", "\(again ? "1" : "2"). Open with Face ID: " + outcome {
                    opened = try keystore.unsealDetailed(blob, reason: "Check that Face ID copies work")
                    return "pass"
                })
                if var value = opened {
                    let same = Data(SHA256.hash(data: value)) == digest
                    wipe(&value)
                    add("compare", "\(again ? "2" : "3"). Same value: \(same ? "pass" : "fail")")
                }
            }

            DispatchQueue.main.async {
                if !again {
                    lastSealed = sealed
                    lastDigest = digest
                }
                faceId = rows
                busy = false
            }
        }
    }
}

/// "pass ..." from the body, or "fail: <raw codes> -> <what the core sees>".
private func outcome(_ body: () throws -> String) -> String {
    do {
        return try body()
    } catch let failure as EnclaveFailure {
        return "fail: \(failure.detail) -> core sees \(coreName(failure.error))"
    } catch {
        return "fail: \(error)"
    }
}

private func coreName(_ error: KeystoreError) -> String {
    switch error {
    case .Cancelled: return "Cancelled"
    case .NotRecognised: return "NotRecognised"
    case .LockedOut: return "LockedOut"
    case .NotEnrolled: return "NotEnrolled"
    case .KeyGone: return "KeyGone"
    case .Failed: return "Failed"
    }
}
#endif
