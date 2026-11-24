#if DEBUG && targetEnvironment(simulator)
import CryptoKit
import SwiftUI

/// The AutoFill screens hosted in the app, for the UI tests: the simulator
/// cannot run a real AutoFill session from another app. Debug simulator
/// builds only.
///
/// - `-pawHostProvider https://github.com`: the list for that site; `app`:
///   an app with no site.
/// - `-pawHostProviderEntry web/github.com`: a tapped suggestion above the
///   keyboard, after what `provideCredentialWithoutUserInteraction` answers.
/// - `-pawHostProviderStore none`: no store set up; `busy`: the store is
///   busy the first time it is opened.
/// - `-pawMatchTable 1`: the site-matching table (`MatchTableView`).
///
/// The fixture store, the simulated key (`-pawSimPin` and the rest) and a
/// simulated Face ID (`-pawSimFaceId`) stand in. What a fill hands over is
/// shown as the user name, the password's length and the start of its
/// SHA-256, never the password.
enum AutoFillHarnessLaunch {
    static var isRequested: Bool {
        LaunchArguments.string("pawHostProvider") != nil || LaunchArguments.string("pawHostProviderEntry") != nil
            || LaunchArguments.bool("pawMatchTable")
    }

    static var request: AutoFillRequest {
        if let path = LaunchArguments.string("pawHostProviderEntry") {
            return .entry(path: path)
        }
        let site = LaunchArguments.string("pawHostProvider") ?? ""
        // `app`: an app iOS gives no site for.
        return .list(services: site == "app" ? [] : [site])
    }

    static var colorScheme: ColorScheme? {
        switch LaunchArguments.string("pawAppearance") {
        case "dark": return .dark
        case "light": return .light
        default: return nil
        }
    }
}

struct AutoFillHarnessRoot: View {
    var body: some View {
        Group {
            if LaunchArguments.bool("pawMatchTable") {
                MatchTableView()
            } else {
                AutoFillHarnessView()
            }
        }
        .preferredColorScheme(AutoFillHarnessLaunch.colorScheme)
    }
}

/// The fixture store, its copies and the key port, kept across runs so a
/// key fill leaves a copy the next run fills from.
private final class HarnessWorld: @unchecked Sendable {
    let opening: AutoFillOpening
    /// The key port behind `opening`, to read whether a PIN is held.
    private(set) var keyPort: KeyPort?
    private let lock = NSLock()
    private var busyOnce = LaunchArguments.string("pawHostProviderStore") == "busy"

    /// What the model's open finds: busy once if asked, then `opening`.
    func open() -> AutoFillOpening {
        lock.withLock {
            if busyOnce {
                busyOnce = false
                return .busy
            }
            return opening
        }
    }

    init() {
        if LaunchArguments.string("pawHostProviderStore") == "none" {
            opening = .noStore
            return
        }
        do {
            let root = FileManager.default.temporaryDirectory
                .appendingPathComponent("paw-autofill-\(UUID().uuidString)", isDirectory: true)
            let storeDir = root.appendingPathComponent("store", isDirectory: true)
            let copiesDir = root.appendingPathComponent("window", isDirectory: true)
            try FileManager.default.createDirectory(at: copiesDir, withIntermediateDirectories: true)
            try writeFixtureStore(dir: storeDir.path)
            let keystore = SimKeystore()
            switch LaunchArguments.string("pawSimFaceId") {
            case "cancel": keystore.setFaceId(answer: .cancel)
            case "nomatch": keystore.setFaceId(answer: .noMatch)
            default: keystore.setFaceId(answer: .match)
            }
            let clock = SimClock(wallMs: Int64(Date().timeIntervalSince1970 * 1000)).asClock()
            let port = KeyPort(clock: clock)
            keyPort = port
            opening = .ready(AutoFillSource(
                store: try PassStore(dir: storeDir.path),
                copies: CopyCache(dir: copiesDir.path, keystore: keystore.asKeystore(), clock: clock),
                keyPort: port,
                connect: { try KeyConnection.openForSession() }))
        } catch {
            opening = .unreadable(String(describing: error))
        }
    }
}

@MainActor @Observable
private final class HarnessRun: AutoFillCompletion {
    private(set) var model: AutoFillModel?
    private(set) var result = "none"
    private(set) var quickType: String?
    @ObservationIgnored private let world = HarnessWorld()

    func run() {
        result = "running"
        let request = AutoFillHarnessLaunch.request
        if case .entry = request {
            quickType = StoreForAutoFill.answerWithoutScreen.rawValue
        }
        let world = world
        model = AutoFillModel(request: request, completion: self, open: { world.open() })
    }

    func fill(user: String, password: String) {
        let digest = SHA256.hash(data: Data(password.utf8)).map { String(format: "%02x", $0) }.joined()
        result = "filled user=\(user) password-length=\(password.count) password-sha=\(digest.prefix(12))"
        model?.shutDown()
        model = nil
    }

    func end(_ reason: AutoFillEnd) {
        result = "ended \(reason.rawValue)"
        model = nil
    }

    /// What the extension's controller does when its view goes away
    /// without paw ending the request (a swipe down): the model ends the
    /// request, except under a Face ID prompt.
    func dismiss() {
        model?.viewWentAway()
        if model != nil {
            model = nil
            result = "dismissed"
        }
    }

    var holdsPin: Bool {
        world.keyPort?.session()?.holdsPin() == true
    }
}

private struct AutoFillHarnessView: View {
    @State private var run = HarnessRun()

    var body: some View {
        Group {
            if let model = run.model {
                AutoFillRootView(model: model)
                    .overlay(alignment: .topLeading) {
                        KeyTasksSignal(tasks: model.pendingKeyTasks)
                    }
                    .overlay(alignment: .topTrailing) {
                        // `-pawHarnessDismiss 1`: stands in for a swipe down.
                        if LaunchArguments.bool("pawHarnessDismiss") {
                            SimulatorActionButton(action: SimulatorAction(
                                title: "swipe the sheet away", identifier: "harness-dismiss") { run.dismiss() })
                                .padding(.top, PawMetrics.Spacing.xxl * 2)
                                .padding(.trailing, PawMetrics.Spacing.l)
                        }
                    }
            } else {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.l) {
                    Text("AutoFill harness")
                        .font(PawFont.title)
                    Text(run.result)
                        .font(PawFont.footnote.monospaced())
                        .accessibilityIdentifier("autofill-result")
                    Button("Touch the simulated key") { KeyConnection.simulateTouch() }
                        .buttonStyle(.pawSecondary)
                        .accessibilityIdentifier("harness-touch-key")
                    Button("Run again") { run.run() }
                        .buttonStyle(.pawPrimary)
                        .accessibilityIdentifier("harness-again")
                    Spacer(minLength: 0)
                }
                .padding(PawMetrics.Spacing.l)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .background(PawColor.bg.ignoresSafeArea())
                .overlay(alignment: .topLeading) {
                    KeyTasksSignal(tasks: 0)
                }
            }
        }
        .overlay(alignment: .bottomLeading) {
            TimelineView(.periodic(from: .now, by: 0.25)) { _ in
                let pin = run.holdsPin ? "held" : "none"
                Text(verbatim: pin)
                    .font(.caption2)
                    .frame(width: 1, height: 1)
                    .opacity(0.02)
                    .accessibilityLabel("PIN")
                    .accessibilityValue(pin)
                    .accessibilityIdentifier("harness-pin")
            }
        }
        .overlay(alignment: .bottomTrailing) {
            // What `provideCredentialWithoutUserInteraction` answered.
            if let quickType = run.quickType {
                Text(verbatim: quickType)
                    .font(.caption2)
                    .frame(width: 1, height: 1)
                    .opacity(0.02)
                    .accessibilityLabel("Answer without a screen")
                    .accessibilityValue(quickType)
                    .accessibilityIdentifier("autofill-quicktype")
            }
        }
        .onAppear {
            if run.result == "none" {
                run.run()
            }
        }
    }
}
/// The site-matching rules against hostile names, run in the app because
/// there is no unit test target: `-pawMatchTable 1`. Writes a throwaway
/// store of empty files (only names matter), runs every case through the
/// same code the extension uses, and says which failed.
struct MatchTableView: View {
    @State private var report: (summary: String, failures: [String])?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
                Text("Site matching")
                    .font(PawFont.title)
                if let report {
                    Text(report.summary)
                        .font(PawFont.body.monospaced())
                        .accessibilityValue(report.summary)
                        .accessibilityIdentifier("match-table-summary")
                    ForEach(report.failures, id: \.self) { failure in
                        Text(failure)
                            .font(PawFont.footnote.monospaced())
                            .foregroundStyle(PawColor.danger)
                    }
                }
            }
            .padding(PawMetrics.Spacing.l)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .task {
            let result = await Task.detached { MatchTable.run() }.value
            report = result
        }
    }
}

enum MatchTable {
    /// Entry names chosen to tempt a careless matcher.
    static let entries = [
        "web/github.com", "github.com/alice", "web/GitHub",
        "web/g\u{0456}thub.com",          // Cyrillic і
        "web/example.com", "web/login.example.com", "web/evil-example.com", "web/example.com.evil.net",
        "sites/example.co.uk", "sites/co.uk",
        "web/alice.github.io", "web/github.io",
        "web/192.168.1.1",
        "web/xn--exmple-cua.com", "web/ex\u{00E4}mple.com",
    ]

    /// Site as iOS might report it, and exactly what must be offered, in order.
    static let cases: [(service: String, expected: [String])] = [
        ("https://github.com/login", ["web/github.com", "github.com/alice"]),
        ("https://GitHub.COM./", ["web/github.com", "github.com/alice"]),
        ("https://github.com:8443/x", ["web/github.com", "github.com/alice"]),
        ("github.com", ["web/github.com", "github.com/alice"]),
        ("https://gist.github.com", ["web/github.com", "github.com/alice"]),
        ("https://github.com.attacker.example/", []),
        ("https://notgithub.com", []),
        ("https://github.com@attacker.example/", []),
        ("https://attacker.example/?next=https://github.com", []),
        ("http://github/", []),
        ("github", []),
        ("https://g\u{0456}thub.com", []),
        ("https://example.com", ["web/example.com", "web/login.example.com"]),
        ("https://login.example.com", ["web/login.example.com", "web/example.com"]),
        ("https://www.login.example.com", ["web/example.com", "web/login.example.com"]),
        ("https://evil-example.com", ["web/evil-example.com"]),
        ("https://example.com.evil.net", ["web/example.com.evil.net"]),
        ("https://example.co.uk", ["sites/example.co.uk"]),
        ("https://shop.example.co.uk", ["sites/example.co.uk"]),
        ("https://other.co.uk", []),
        ("https://bob.github.io", []),
        ("https://alice.github.io/x", ["web/alice.github.io"]),
        ("http://192.168.1.1:8080/admin", ["web/192.168.1.1"]),
        ("http://192.168.1.10", []),
        ("https://xn--exmple-cua.com", ["web/xn--exmple-cua.com"]),
        ("https://ex\u{00E4}mple.com", []),
        ("https://[::1]/", []),
        ("", []),
    ]

    /// The suggestion each entry gives above the keyboard: domain and user
    /// string, or nil for none.
    static let identities: [(path: String, expected: String?)] = [
        ("web/github.com", "github.com github.com"),
        ("github.com/alice", "github.com alice"),
        ("web/GitHub", nil),
        ("web/g\u{0456}thub.com", nil),
        ("web/ex\u{00E4}mple.com", nil),
        ("web/192.168.1.1", nil),
        ("sites/co.uk", nil),
        ("web/github.io", nil),
        ("sites/example.co.uk", "example.co.uk example.co.uk"),
        ("web/xn--exmple-cua.com", "xn--exmple-cua.com xn--exmple-cua.com"),
    ]

    static func run() -> (summary: String, failures: [String]) {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("paw-match-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let store: PassStore
        do {
            for entry in entries {
                let file = root.appendingPathComponent(entry + ".gpg")
                try FileManager.default.createDirectory(
                    at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
                try Data("not encrypted".utf8).write(to: file)
            }
            try Data("0000\n".utf8).write(to: root.appendingPathComponent(".gpg-id"))
            store = try PassStore(dir: root.path)
        } catch {
            return ("could not make the store: \(error)", [])
        }
        var failures: [String] = []
        for test in cases {
            let got = SiteMatch.entries(in: store, for: [test.service]).map(\.path)
            if got != test.expected {
                failures.append("\(test.service.debugDescription): got \(got), want \(test.expected)")
            }
        }
        let made = CredentialIdentities.make(from: store)
        for test in identities {
            let got = made.first { $0.path == test.path }.map { "\($0.domain) \($0.user)" }
            if got != test.expected {
                failures.append("identity \(test.path): got \(got ?? "none"), want \(test.expected ?? "none")")
            }
        }
        let total = cases.count + identities.count
        let summary = failures.isEmpty ? "all \(total) passed" : "\(failures.count) of \(total) failed"
        return (summary, failures)
    }
}
#endif
