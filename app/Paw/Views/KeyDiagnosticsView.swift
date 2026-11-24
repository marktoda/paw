import SwiftUI
import UIKit

/// Settings › Key diagnostics on a set-up store: what the key session read
/// when the key was plugged in. It asks for no PIN and sends nothing to the
/// key, so it cannot get round the enrolled-key and PIN KDF checks that
/// every key task makes. (The spike's PIN test screen is debug-only.)
///
/// Below that, the key log: the card commands and key sheet stages of the
/// app and the AutoFill extension, newest first, to copy into a report.
struct KeyDiagnosticsView: View {
    let model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var logLines: [KeyLogLine] = []
    @State private var logProblem: String?
    @State private var logCopied = false

    /// The most lines shown; Copy log takes them all.
    private static let shownLines = 300

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
                             ? "The key this store was set up with."
                             : "Not the key this store was set up with.")
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
                keyLogSection
            }
            .listStyle(.insetGrouped)
            .onAppear(perform: loadLog)
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

    // MARK: Key log

    @ViewBuilder private var keyLogSection: some View {
        Section {
            if let logProblem {
                ErrorRow(kind: .error, problem: "The key log wasn't cleared", nextStep: logProblem)
            }
            if logLines.isEmpty {
                Text("Nothing yet. Open a password with your key.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("key-log-empty")
            } else {
                Button(logCopied ? "Log copied" : "Copy log", action: copyLog)
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("key-log-copy")
                Button("Clear", role: .destructive, action: clearLog)
                    .accessibilityIdentifier("key-log-clear")
                ForEach(Array(logLines.suffix(Self.shownLines).reversed().enumerated()), id: \.offset) { _, line in
                    Text("\(line.time) \(line.process) \(line.text)")
                        .font(PawFont.caption.monospaced())
                        .foregroundStyle(PawColor.labelSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                        .accessibilityIdentifier("key-log-line")
                }
            }
        } header: {
            PawSectionHeader("Key log")
        } footer: {
            Text("What paw and your key said, in this app and in AutoFill. No PIN, passwords or names.")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
        }
    }

    private var logDir: String? {
        model.keyPort.keyLogDir()
    }

    private var utcOffset: Int32 {
        Int32(TimeZone.current.secondsFromGMT())
    }

    private func loadLog() {
        guard let logDir else { return }
        logLines = keyLog(stateDir: logDir, utcOffsetSecs: utcOffset)
    }

    private func copyLog() {
        guard let logDir else { return }
        // Plain pasteboard, not `Clipboard`: the owner pastes this on a
        // desktop, so it is not limited to this device or to 45 seconds.
        // The key log holds no secrets: instruction bytes, status words,
        // timings, stages and error names only; never command or response
        // data, the PIN or its length, entry or host names
        // (core/src/key/keylog.rs, checked by core/tests/key_log.rs).
        UIPasteboard.general.string = keyLogText(stateDir: logDir, utcOffsetSecs: utcOffset)
        logCopied = true
    }

    private func clearLog() {
        guard let logDir else { return }
        do {
            try clearKeyLog(stateDir: logDir)
            logProblem = nil
        } catch {
            logProblem = String(describing: error)
        }
        loadLog()
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
