#if DEBUG && targetEnvironment(simulator)
import AuthenticationServices
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
/// The requests go through `AutoFillProvider`, as the extension's do, on
/// the fixture store laid out as `StoreLocation` lays a store out. The
/// simulated key (`-pawSimPin` and the rest) and a simulated Face ID
/// (`-pawSimFaceId`) stand in. What a fill hands over is
/// shown as the user name, the password's length and the start of its
/// SHA-256, never the password; a code likewise, by length and SHA-256.
enum AutoFillHarnessLaunch {
    static var isRequested: Bool {
        LaunchArgument.hostProvider.string != nil || LaunchArgument.hostProviderEntry.string != nil
    }

    /// What iOS would ask for.
    enum Ask {
        case list([ASCredentialServiceIdentifier], AutoFillKind)
        case entry(ASCredentialRequest)
    }

    static func request(run: Int) -> Ask {
        if let path = LaunchArgument.hostProviderEntry.string {
            let (kind, bare) = parse(path)
            return .entry(suggestion(kind, path: bare))
        }
        let sites = (LaunchArgument.hostProvider.string ?? "").split(separator: ",").map(String.init)
        let (kind, site) = parse(sites.isEmpty ? "" : sites[min(run, sites.count - 1)])
        // `app`: an app iOS gives no site for.
        return .list(site == "app" ? [] : [ASCredentialServiceIdentifier(identifier: site, type: .URL)], kind)
    }

    /// A tap on the suggestion for `path`, as iOS hands it over. iOS gives
    /// no site with it here.
    private static func suggestion(_ kind: AutoFillKind, path: String) -> ASCredentialRequest {
        let identity = kind.identity(domain: "", user: EntrySite.suggestionUser(forPath: path), path: path)
        switch identity {
        case let password as ASPasswordCredentialIdentity:
            return ASPasswordCredentialRequest(credentialIdentity: password)
        case let code as ASOneTimeCodeCredentialIdentity:
            return ASOneTimeCodeCredentialRequest(credentialIdentity: code)
        default:
            preconditionFailure("paw publishes no other kind of suggestion")
        }
    }

    /// `code:…` asks for a one-time code; anything else for a password.
    private static func parse(_ value: String) -> (AutoFillKind, String) {
        value.hasPrefix("code:") ? (.oneTimeCode, String(value.dropFirst(5))) : (.password, value)
    }
}

struct AutoFillHarnessRoot: View {
    var body: some View {
        AutoFillHarnessView()
            .preferredColorScheme(LaunchArgument.colorScheme)
    }
}

/// The fixture store, its copies and the key port, kept across runs so a
/// key fill leaves a copy the next run fills from.
private final class HarnessWorld: @unchecked Sendable {
    let opening: AutoFillOpening
    /// What `opening` reads: the key port, to read whether a PIN is held,
    /// and the site pairings and code list, to show what was recorded.
    private(set) var source: AutoFillSource?
    private let lock = NSLock()
    private var busyOnce = LaunchArgument.hostProviderStore.string == "busy"

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
        if LaunchArgument.hostProviderStore.string == "none" {
            opening = .noStore
            return
        }
        do {
            let location = StoreLocation(root: FileManager.default.temporaryDirectory
                .appendingPathComponent("paw-autofill-\(UUID().uuidString)", isDirectory: true))
            try FileManager.default.createDirectory(at: location.copiesDir, withIntermediateDirectories: true)
            try FileManager.default.createDirectory(at: location.stateDir, withIntermediateDirectories: true)
            try writeFixtureStore(dir: location.repoDir.path)
            // `finance/northwind` and `contoso/alice`: named without a domain.
            try writeNameMatchEntries(dir: location.repoDir.path)
            let keystore = SimKeystore()
            switch LaunchArgument.simFaceId.string {
            case "cancel": keystore.setFaceId(answer: .cancel)
            case "nomatch": keystore.setFaceId(answer: .noMatch)
            default: keystore.setFaceId(answer: .match)
            }
            let clock = SimClock(wallMs: Int64(Date().timeIntervalSince1970 * 1000)).asClock()
            let source = try AutoFillSource.open(
                at: location, clock: clock, connect: { try KeyConnection.open() },
                copies: { CopyCache(dir: $0.path, keystore: keystore.asKeystore(), clock: clock) })
            if LaunchArgument.simOneTimeCodes.bool {
                try source.codes?.setEnabled(enabled: true)
            }
            opening = .ready(source)
            self.source = source
        } catch {
            opening = .unreadable(String(describing: error))
        }
    }
}

@MainActor @Observable
private final class HarnessRun: AutoFillAnswers {
    private(set) var model: AutoFillModel?
    private(set) var result = "none"
    private(set) var quickType: String?
    /// What `SitePairings` holds: "entry=site …", or "none".
    private(set) var pairings = "none"
    /// The entries `OneTimeCodeEntries` knows have a code, or "none".
    private(set) var codes = "none"
    @ObservationIgnored private let world: HarnessWorld
    @ObservationIgnored private let provider: AutoFillProvider
    @ObservationIgnored private var runs = 0

    init() {
        let world = HarnessWorld()
        self.world = world
        provider = AutoFillProvider(open: { world.open() })
        provider.answers = self
    }

    func run() {
        result = "running"
        let ask = AutoFillHarnessLaunch.request(run: runs)
        runs += 1
        switch ask {
        case .list(let services, let kind):
            model = provider.list(services, kind: kind)
        case .entry(let request):
            quickType = AutoFillProvider.answerWithoutScreen(request).rawValue
            model = provider.entry(request)
        }
    }

    func complete(with credential: AutoFillCredential) {
        switch credential {
        case .password(let user, let password):
            result = "filled user=\(user) password-length=\(password.count) password-sha=\(Self.digest(password))"
        case .oneTimeCode(let code):
            result = "filled code-length=\(code.count) code-sha=\(Self.digest(code))"
        }
        model = nil
        readPairingsSoon()
    }

    func cancel(_ reason: AutoFillEnd) {
        result = "ended \(reason.rawValue)"
        model = nil
        readPairings()
    }

    private static func digest(_ text: String) -> String {
        String(SHA256.hash(data: Data(text.utf8)).map { String(format: "%02x", $0) }.joined().prefix(12))
    }

    /// Once the model's fill has returned: it keeps a code mark after
    /// handing the fill over.
    private func readPairingsSoon() {
        Task { @MainActor [weak self] in
            self?.readPairings()
        }
    }

    private func readPairings() {
        let source = world.source
        let all = (source?.pairings?.pairings() ?? []).flatMap { p in p.sites.map { "\(p.entry)=\($0)" } }
        pairings = all.isEmpty ? "none" : all.joined(separator: " ")
        let known = source.map { $0.codes?.entries(store: $0.store) ?? [] } ?? []
        codes = known.isEmpty ? "none" : known.joined(separator: " ")
    }

    /// The extension's view going away without paw ending the request (a
    /// swipe down): the request ends, except under a Face ID prompt, whose
    /// answer still fills.
    func dismiss() {
        provider.viewWentAway()
        if model != nil {
            model = nil
            result = "dismissed"
        }
    }

    var holdsPin: Bool {
        world.source?.keyPort.session()?.holdsPin() == true
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
                        if LaunchArgument.harnessDismiss.bool {
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
