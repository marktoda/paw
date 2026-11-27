import SwiftUI
import UIKit

/// Settings › Key diagnostics on a set-up store: what the key session read
/// when the key was plugged in, which asks for no PIN and sends nothing to
/// the key. Then the touch timeout check (`TouchTimeoutSection`), a key task
/// like any other, so it cannot get round the enrolled-key and PIN KDF
/// checks every key task makes.
///
/// Below that, the key log: the card commands and key sheet stages of the
/// app and the AutoFill extension, newest first, to copy into a report.
struct KeyDiagnosticsView: View {
    let model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var logLines: [KeyLogLine] = []
    @State private var logProblem: String?
    @State private var logCopied = false
    /// What Copy diagnostics put on the pasteboard, shown below it.
    @State private var copiedReport: String?
    @State private var assembling = false
    @State private var touchCheck: TouchTimeoutCheck

    /// The most lines shown; Copy log takes them all.
    private static let shownLines = 300

    init(model: AppModel) {
        self.model = model
        _touchCheck = State(initialValue: TouchTimeoutCheck(session: model.storeSession))
    }

    private var readerNames: [String] {
        model.storeSession.reader.source.readerNames
    }

    var body: some View {
        NavigationStack {
            List {
                if let session = model.storeSession.keyPort.session() {
                    let info = session.info()
                    let kdf = session.pinKdf()
                    Section {
                        row("Serial", spokenSerial(info.ident), identifier: "diagnostics-serial")
                        row("Opening key", info.decryptionFingerprint.map(fingerprintEnding).map { "ends \($0)" } ?? "None",
                            identifier: "diagnostics-decryption")
                        row("SSH key", info.authenticationFingerprint.map(fingerprintEnding).map { "ends \($0)" } ?? "None",
                            identifier: "diagnostics-ssh")
                        row("PIN tries left", "\(info.pinTriesLeft)", identifier: "diagnostics-tries")
                        row("Touch to open", Self.words(info.decryptionTouch), identifier: "diagnostics-touch-open")
                        row("Touch to sign in", Self.words(info.authenticationTouch), identifier: "diagnostics-touch-ssh")
                        row("PIN hashing", kdf.enabled ? (kdf.supported ? "On" : "On, not supported") : "Off",
                            identifier: "diagnostics-kdf")
                    } header: {
                        PawSectionHeader("Plugged in")
                    }
                    Section {
                        Text(model.storeSession.keyPort.keyMatchesEnrolment()
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
                                model.storeSession.reader.read()
                            }
                        }
                    }
                }
                Section {
                    row("Connected", readerNames.isEmpty ? "None" : readerNames.joined(separator: ", "),
                        identifier: "diagnostics-readers")
                }
                reportSection
                TouchTimeoutSection(model: model, check: touchCheck, finished: loadLog)
                keyLogSection
            }
            .listStyle(.insetGrouped)
            .onAppear(perform: loadLog)
            .onDisappear { touchCheck.cancel() }
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

    // MARK: Copy diagnostics

    @ViewBuilder private var reportSection: some View {
        Section {
            Button("Copy diagnostics", action: copyReport)
                .foregroundStyle(PawColor.label)
                .disabled(assembling)
                .accessibilityIdentifier("diagnostics-copy")
            if let copiedReport {
                CopiedLine(identifier: "diagnostics-copied")
                Text(copiedReport)
                    .font(PawFont.caption.monospaced())
                    .foregroundStyle(PawColor.labelSecondary)
                    .lineLimit(24)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                    .accessibilityIdentifier("diagnostics-report")
            }
        } footer: {
            Text("No names, addresses or fingerprints.")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
        }
    }

    /// The core writes the text (core/src/report.rs, checked by
    /// core/tests/settings.rs); the app gives only what it alone knows.
    private func copyReport() {
        let facts = DiagnosticsFacts(
            appVersion: Self.bundleString("CFBundleShortVersionString"),
            appBuild: Self.bundleString("CFBundleVersion"),
            systemVersion: "\(UIDevice.current.systemName) \(UIDevice.current.systemVersion)",
            deviceModel: Self.deviceModel,
            lastSync: model.storeSession.syncing?.pillState.lastSync,
            lastCheckedMs: model.storeSession.syncing?.lastChecked.map { Int64($0.timeIntervalSince1970 * 1000) })
        let stateDir = model.storeSession.stateDir
        let port = model.storeSession.keyPort
        let github = model.storeSession.gitHub?.core
        let copies = model.copies.cache
        assembling = true
        Task {
            // Reads the store, the queue and the keychain: off the main thread.
            let text = await Task.detached(priority: .userInitiated) {
                diagnosticsText(facts: facts, stateDir: stateDir, port: port, github: github, copies: copies)
            }.value
            assembling = false
            Self.copyForDesktop(text)
            copiedReport = text
        }
    }

    private static func bundleString(_ key: String) -> String {
        Bundle.main.object(forInfoDictionaryKey: key) as? String ?? "not recorded"
    }

    /// The model identifier, e.g. "iPhone17,1"; the simulator's says which
    /// device it simulates.
    private static var deviceModel: String {
        if let simulated = ProcessInfo.processInfo.environment["SIMULATOR_MODEL_IDENTIFIER"] {
            return simulated
        }
        var info = utsname()
        uname(&info)
        return withUnsafeBytes(of: &info.machine) { raw in
            String(decoding: raw.prefix(while: { $0 != 0 }), as: UTF8.self)
        }
    }

    // MARK: Key log

    @ViewBuilder private var keyLogSection: some View {
        Section {
            if let logProblem {
                ErrorRow(kind: .error, problem: "The key log wasn't cleared", nextStep: logProblem)
            }
            if logLines.isEmpty {
                Text("Nothing yet.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("key-log-empty")
            } else {
                Button("Copy log", action: copyLog)
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("key-log-copy")
                if logCopied {
                    CopiedLine(identifier: "key-log-copied")
                }
                Button("Clear", role: .destructive, action: clearLog)
                    .foregroundStyle(PawColor.danger)
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
            Text("No PINs, passwords or names.")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
        }
    }

    private var utcOffset: Int32 {
        Int32(TimeZone.current.secondsFromGMT())
    }

    private func loadLog() {
        logLines = model.storeSession.keyPort.keyLog(utcOffsetSecs: utcOffset)
    }

    private func copyLog() {
        // The key log holds no secrets: instruction bytes, status words,
        // timings, stages and error names only; never command or response
        // data, the PIN or its length, entry or host names
        // (core/src/key/keylog.rs, checked by core/tests/key_log.rs).
        Self.copyForDesktop(model.storeSession.keyPort.keyLogText(utcOffsetSecs: utcOffset))
        logCopied = true
    }

    /// How long a copied report or log stays on the pasteboard.
    static let copyLifetime: TimeInterval = 10 * 60

    /// Not as a secret: the owner pastes these on a desktop, so they may go
    /// to other devices by Universal Clipboard (principles 6 notes the
    /// exception), but they still expire, after 10 minutes, not 45 seconds.
    private static func copyForDesktop(_ text: String) {
        Clipboard.copyNotSecret(text, localOnly: false, expiry: Date().addingTimeInterval(copyLifetime))
    }

    private func clearLog() {
        do {
            try model.storeSession.keyPort.clearKeyLog()
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

/// "Copied. Clears in 10 minutes.", under a Copy button.
private struct CopiedLine: View {
    let identifier: String

    var body: some View {
        Text("Copied. Clears in \(Int(KeyDiagnosticsView.copyLifetime / 60)) minutes.")
            .font(PawFont.footnote)
            .foregroundStyle(PawColor.labelSecondary)
            .accessibilityIdentifier(identifier)
    }
}
