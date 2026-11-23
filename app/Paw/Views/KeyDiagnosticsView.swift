import SwiftUI

/// Settings › Key diagnostics on a set-up store: what the key session read
/// when the key was plugged in. It asks for no PIN and sends nothing to the
/// key, so it cannot get round the enrolled-key and PIN KDF checks that
/// every key task makes. (The spike's PIN test screen is debug-only.)
struct KeyDiagnosticsView: View {
    let model: AppModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                if let session = model.keyPort.session() {
                    let info = session.info()
                    let kdf = session.pinKdf()
                    Section {
                        row("Serial", spokenSerial(info.ident), identifier: "diagnostics-serial")
                        row("Decryption key", info.decryptionFingerprint.map(fingerprintEnding).map { "ends \($0)" } ?? "None",
                            identifier: "diagnostics-decryption")
                        row("SSH key", info.authenticationFingerprint.map(fingerprintEnding).map { "ends \($0)" } ?? "None",
                            identifier: "diagnostics-ssh")
                        row("PIN tries left", "\(info.pinTriesLeft)", identifier: "diagnostics-tries")
                        row("Touch to open", Self.words(info.decryptionTouch), identifier: "diagnostics-touch-open")
                        row("Touch to sign in", Self.words(info.authenticationTouch), identifier: "diagnostics-touch-ssh")
                        row("PIN setting (KDF)", kdf.enabled ? (kdf.supported ? "On" : "On, not supported") : "Off",
                            identifier: "diagnostics-kdf")
                    } header: {
                        PawSectionHeader("Plugged in")
                    }
                    Section {
                        Text(model.keyPort.keyMatchesEnrolment()
                             ? "This is the key this store was set up with."
                             : "This is a different key from the one this store was set up with.")
                            .font(PawFont.subheadline)
                            .foregroundStyle(PawColor.label)
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityIdentifier("diagnostics-enrolment")
                    }
                } else {
                    Section {
                        Text("No key plugged in.")
                            .font(PawFont.subheadline)
                            .foregroundStyle(PawColor.labelSecondary)
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityIdentifier("diagnostics-no-key")
                        if let problem = model.attachProblem {
                            ErrorRow(kind: .error, problem: problem.problem, nextStep: problem.nextStep,
                                     actionTitle: "Try again", actionNeedsKey: true) {
                                model.plugIn()
                            }
                        }
                    }
                }
                Section {
                    row("Readers", KeyConnection.readerNames.isEmpty ? "None" : KeyConnection.readerNames.joined(separator: ", "),
                        identifier: "diagnostics-readers")
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle("Key diagnostics")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                        .accessibilityIdentifier("diagnostics-done")
                }
            }
        }
    }

    private func row(_ label: String, _ value: String, identifier: String) -> some View {
        LabeledContent {
            Text(value)
                .font(PawFont.footnote.monospacedDigit())
                .foregroundStyle(PawColor.labelSecondary)
                .multilineTextAlignment(.trailing)
        } label: {
            Text(label)
                .foregroundStyle(PawColor.label)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(identifier)
    }

    private static func words(_ touch: KeyTouch) -> String {
        switch touch {
        case .off: "Never"
        case .eachUse: "Every time"
        case .cached: "Once per 15 seconds"
        case .unknown: "Unknown"
        }
    }
}
