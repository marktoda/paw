import SwiftUI

#if DEBUG
/// The spike's key test, for debug builds only (the simulator's KeyTests and
/// a developer's own checks): it opens its own card connection and sends a
/// typed PIN outside the key session, so none of the session's checks (the
/// enrolled key, the PIN KDF) apply. Release builds don't contain it; a
/// set-up store's Settings shows `KeyDiagnosticsView` instead.
///
/// One screen that exercises the key. `host` says which process is running
/// it, since the point is to compare the app with the AutoFill extension.
struct SelfTestView: View {
    let host: String

    @State private var pin = ""
    @State private var results: [String] = []
    @State private var busy = false
    @State private var waitingForTouch = false

    var body: some View {
        NavigationStack {
            List {
                Section("Key") {
                    Button("Read key") { run { try describeKey() } }
                        .accessibilityIdentifier("read-key")
                        .disabled(busy)
                }
                Section("Decrypt") {
                    // As the key sheet's field: no content type, so iOS
                    // offers no password AutoFill and no "Save Password?".
                    SecureField("Key PIN", text: $pin)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                        .accessibilityIdentifier("pin-field")
                        .disabled(busy)
                    Button("Decrypt a test message") {
                        let pin = pin
                        run {
                            let text = try KeySelfTest.roundTrip(pin: pin) {
                                DispatchQueue.main.async { waitingForTouch = true }
                            }
                            return text == KeySelfTest.message
                                ? "Decrypted correctly."
                                : "Decrypted to something unexpected: \(text)"
                        }
                    }
                    .accessibilityIdentifier("decrypt")
                    .disabled(busy || pin.isEmpty)

                    if waitingForTouch {
                        Label("Touch your key", systemImage: "circle.fill")
                            .foregroundStyle(.orange)
                            .accessibilityIdentifier("touch-prompt")
                        if KeyConnection.isSimulated {
                            Button("Touch the simulated key") { KeyConnection.simulateTouch() }
                                .accessibilityIdentifier("simulated-touch")
                        }
                    }
                }
                Section {
                    Button("Run card checks (three touches)") {
                        let pin = pin
                        run {
                            try KeySelfTest.runChecks(pin: pin) {
                                DispatchQueue.main.async { waitingForTouch = true }
                            }
                        }
                    }
                    .accessibilityIdentifier("card-checks")
                    .disabled(busy || pin.isEmpty)
                    Button("Check the touch timeout (do not touch)") {
                        let pin = pin
                        run { try KeySelfTest.runTouchTimeoutCheck(pin: pin) }
                    }
                    .accessibilityIdentifier("touch-timeout-check")
                    .disabled(busy || pin.isEmpty)
                } header: {
                    Text("Card checks")
                } footer: {
                    Text("Uses the PIN above. Results show status words and timings, never the PIN.")
                }
                Section("Results, newest first") {
                    if results.isEmpty {
                        Text("Nothing yet.").foregroundStyle(.secondary)
                    }
                    ForEach(Array(results.enumerated()), id: \.offset) { index, line in
                        Text(line)
                            .font(.footnote.monospaced())
                            .textSelection(.enabled)
                            .accessibilityIdentifier(index == 0 ? "latest-result" : "older-result")
                    }
                }
            }
            .navigationTitle("Paw key test (\(host))")
            .navigationBarTitleDisplayMode(.inline)
        }
    }

    private func describeKey() throws -> String {
        let key = try KeySelfTest.readKey()
        return """
            Reader: \(KeyConnection.readerNames.joined(separator: ", "))
            Key: \(key.ident)
            Decryption key: \(key.decryptionFingerprint ?? "none")
            PIN tries left: \(key.pinTriesLeft)
            Touch to decrypt: \(Self.words(key.decryptionTouch))
            Touch to log in (SSH): \(Self.words(key.authenticationTouch))
            SSH key: \(key.authenticationFingerprint ?? "none")
            """
    }

    private static func words(_ touch: KeyTouch) -> String {
        switch touch {
        case .off: return "never"
        case .eachUse: return "every time"
        case .cached: return "once per 15 seconds"
        case .unknown: return "unknown"
        }
    }

    private func run(_ work: @escaping @Sendable () throws -> String) {
        busy = true
        // Each run has already captured the PIN it needs: the field empties.
        pin = ""
        let started = Date()
        DispatchQueue.global(qos: .userInitiated).async {
            let outcome: String
            do {
                outcome = try work()
            } catch {
                outcome = describe(error)
            }
            let seconds = String(format: "%.1f", Date().timeIntervalSince(started))
            DispatchQueue.main.async {
                results.insert("\(outcome)\n(\(host), \(seconds)s)", at: 0)
                busy = false
                waitingForTouch = false
            }
        }
    }
}
#endif
