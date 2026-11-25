#if DEBUG && targetEnvironment(simulator)
import CryptoKit
import SwiftUI

/// The AutoFill screens hosted in the app, for the UI tests: the simulator
/// cannot run a real AutoFill session from another app. Debug simulator
/// builds only.
///
/// - `-pawHostProvider https://github.com`: the list for that site; `app`:
///   an app with no site. Several sites, comma-separated: one per run
///   ("Run again" moves to the next and stays on the last), sharing the
///   copies and the site pairings.
/// - `-pawHostProviderEntry web/github.com`: a tapped suggestion above the
///   keyboard, after what `provideCredentialWithoutUserInteraction` answers.
/// - `code:` before a site or an entry (`code:https://github.com`): a
///   one-time-code request instead of a password one, for that run.
/// - `-pawHostProviderStore none`: no store set up; `busy`: the store is
///   busy the first time it is opened.
/// - `-pawSimOneTimeCodes 1`: "Suggest one-time codes" on, as Settings
///   turns it on; off by default, as on a phone.
///
/// The fixture store, the simulated key (`-pawSimPin` and the rest) and a
/// simulated Face ID (`-pawSimFaceId`) stand in. What a fill hands over is
/// shown as the user name, the password's length and the start of its
/// SHA-256, never the password; a code likewise, by length and SHA-256.
enum AutoFillHarnessLaunch {
    static var isRequested: Bool {
        LaunchArguments.string("pawHostProvider") != nil || LaunchArguments.string("pawHostProviderEntry") != nil
    }

    static func request(run: Int) -> (AutoFillRequest, AutoFillKind) {
        if let path = LaunchArguments.string("pawHostProviderEntry") {
            let (kind, bare) = parse(path)
            return (.entry(path: bare), kind)
        }
        let sites = (LaunchArguments.string("pawHostProvider") ?? "").split(separator: ",").map(String.init)
        let (kind, site) = parse(sites.isEmpty ? "" : sites[min(run, sites.count - 1)])
        // `app`: an app iOS gives no site for.
        return (.list(services: site == "app" ? [] : [site]), kind)
    }

    /// `code:…` asks for a one-time code; anything else for a password.
    private static func parse(_ value: String) -> (AutoFillKind, String) {
        value.hasPrefix("code:") ? (.oneTimeCode, String(value.dropFirst(5))) : (.password, value)
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
        AutoFillHarnessView()
            .preferredColorScheme(AutoFillHarnessLaunch.colorScheme)
    }
}

/// The fixture store, its copies and the key port, kept across runs so a
/// key fill leaves a copy the next run fills from.
private final class HarnessWorld: @unchecked Sendable {
    let opening: AutoFillOpening
    /// The key port behind `opening`, to read whether a PIN is held.
    private(set) var keyPort: KeyPort?
    /// The site pairings behind `opening`, to show what was recorded.
    private(set) var pairings: SitePairings?
    /// Which entries are known to have a code, likewise.
    private(set) var codes: OneTimeCodeEntries?
    private(set) var store: PassStore?
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
            let stateDir = root.appendingPathComponent("state", isDirectory: true)
            try FileManager.default.createDirectory(at: copiesDir, withIntermediateDirectories: true)
            try FileManager.default.createDirectory(at: stateDir, withIntermediateDirectories: true)
            try writeFixtureStore(dir: storeDir.path)
            // `finance/northwind` and `contoso/alice`: named without a domain.
            try writeNameMatchEntries(dir: storeDir.path)
            let pairings = SitePairings.open(stateDir: stateDir.path)
            self.pairings = pairings
            let codes = OneTimeCodeEntries.open(stateDir: stateDir.path)
            if LaunchArguments.bool("pawSimOneTimeCodes") {
                try codes.setEnabled(enabled: true)
            }
            self.codes = codes
            let keystore = SimKeystore()
            switch LaunchArguments.string("pawSimFaceId") {
            case "cancel": keystore.setFaceId(answer: .cancel)
            case "nomatch": keystore.setFaceId(answer: .noMatch)
            default: keystore.setFaceId(answer: .match)
            }
            let clock = SimClock(wallMs: Int64(Date().timeIntervalSince1970 * 1000)).asClock()
            let port = KeyPort(clock: clock)
            keyPort = port
            let store = try PassStore(dir: storeDir.path)
            self.store = store
            opening = .ready(AutoFillSource(
                store: store,
                copies: CopyCache(dir: copiesDir.path, keystore: keystore.asKeystore(), clock: clock),
                keyPort: port,
                connect: { try KeyConnection.open() },
                pairings: pairings,
                codes: codes))
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
    /// What `SitePairings` holds: "entry=site …", or "none".
    private(set) var pairings = "none"
    /// The entries `OneTimeCodeEntries` knows have a code, or "none".
    private(set) var codes = "none"
    @ObservationIgnored private let world = HarnessWorld()
    @ObservationIgnored private var runs = 0

    func run() {
        result = "running"
        let (request, kind) = AutoFillHarnessLaunch.request(run: runs)
        runs += 1
        if case .entry = request {
            quickType = StoreForAutoFill.answerWithoutScreen.rawValue
        }
        let world = world
        model = AutoFillModel(request: request, kind: kind, completion: self, open: { world.open() })
    }

    func fill(user: String, password: String) {
        let digest = SHA256.hash(data: Data(password.utf8)).map { String(format: "%02x", $0) }.joined()
        result = "filled user=\(user) password-length=\(password.count) password-sha=\(digest.prefix(12))"
        model = nil
        readPairingsSoon()
    }

    func fill(oneTimeCode: String) {
        let digest = SHA256.hash(data: Data(oneTimeCode.utf8)).map { String(format: "%02x", $0) }.joined()
        result = "filled code-length=\(oneTimeCode.count) code-sha=\(digest.prefix(12))"
        model = nil
        readPairingsSoon()
    }

    func end(_ reason: AutoFillEnd) {
        result = "ended \(reason.rawValue)"
        model = nil
        readPairings()
    }

    /// Once the model's fill has returned: it keeps a code mark after
    /// handing the fill over.
    private func readPairingsSoon() {
        Task { @MainActor [weak self] in
            self?.readPairings()
        }
    }

    private func readPairings() {
        let all = (world.pairings?.pairings() ?? []).flatMap { p in p.sites.map { "\(p.entry)=\($0)" } }
        pairings = all.isEmpty ? "none" : all.joined(separator: " ")
        let known = world.store.map { world.codes?.entries(store: $0) ?? [] } ?? []
        codes = known.isEmpty ? "none" : known.joined(separator: " ")
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
                        KeyTasksSignal(tasks: model.keyTask.pendingTasks)
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
                    Text(verbatim: "pairings: \(run.pairings)")
                        .font(PawFont.footnote.monospaced())
                        .accessibilityValue(run.pairings)
                        .accessibilityIdentifier("harness-pairings")
                    Text(verbatim: "codes: \(run.codes)")
                        .font(PawFont.footnote.monospaced())
                        .accessibilityValue(run.codes)
                        .accessibilityIdentifier("harness-codes")
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
#endif
