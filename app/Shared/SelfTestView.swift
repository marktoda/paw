import SwiftUI

/// One screen that exercises the key. `host` says which process is running
/// it, since the point is to compare the app with the AutoFill extension.
struct SelfTestView: View {
    let host: String

    @State private var pin = ""
    @State private var log: [String] = []
    @State private var busy = false
    @State private var waitingForTouch = false

    var body: some View {
        NavigationStack {
            List {
                Section("Key") {
                    Button("Read key") { run("Read key") { try describeKey() } }
                }
                Section("Decrypt") {
                    SecureField("Key PIN", text: $pin)
                        .textContentType(.password)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    Button("Encrypt and decrypt a test message") {
                        let pin = pin
                        run("Decrypt") {
                            let text = try KeySelfTest.roundTrip(pin: pin) {
                                DispatchQueue.main.async { waitingForTouch = true }
                            }
                            return text == KeySelfTest.message
                                ? "Decrypted correctly: \(text)"
                                : "Decrypted to something unexpected: \(text)"
                        }
                    }
                    .disabled(pin.isEmpty)
                    if waitingForTouch {
                        Label("Touch your key", systemImage: "circle.fill")
                            .foregroundStyle(.orange)
                    }
                }
                Section("Log") {
                    if log.isEmpty {
                        Text("Nothing yet.").foregroundStyle(.secondary)
                    }
                    ForEach(Array(log.enumerated()), id: \.offset) { _, line in
                        Text(line).font(.footnote.monospaced()).textSelection(.enabled)
                    }
                }
            }
            .navigationTitle("Paw key test (\(host))")
            .navigationBarTitleDisplayMode(.inline)
            .disabled(busy)
        }
    }

    private func describeKey() throws -> String {
        let readers = UsbCardConnection.slotNames()
        let key = try KeySelfTest.readKey()
        return """
            readers: \(readers)
            key: \(key.ident)
            decryption key: \(key.decryptionFingerprint ?? "none")
            PIN tries left: \(key.pinTriesLeft)
            touch required: \(key.touchRequired)
            """
    }

    private func run(_ name: String, _ work: @escaping @Sendable () throws -> String) {
        busy = true
        let started = Date()
        DispatchQueue.global(qos: .userInitiated).async {
            let outcome: String
            do {
                outcome = try work()
            } catch {
                outcome = "FAILED: \(error.localizedDescription) [\(error)]"
            }
            let seconds = String(format: "%.1f", Date().timeIntervalSince(started))
            DispatchQueue.main.async {
                log.insert("\(name) (\(host), \(seconds)s)\n\(outcome)", at: 0)
                busy = false
                waitingForTouch = false
            }
        }
    }
}
