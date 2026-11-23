import SwiftUI

#if targetEnvironment(simulator)
/// `-pawFixture 1`: the app on a made-up store, with the simulated key
/// (`-pawSimPin`, `-pawSimTouch`, `-pawSimTouchTimeoutMs`, `-pawSimPinTries`,
/// `-pawSimUnplugged`), a simulated
/// Face ID (`-pawSimFaceId ok|cancel|nomatch|lockout|notenrolled`) and a
/// simulated clock that the settings sheet can move on, and that moves on
/// by `-pawSimAwayMs` each time paw goes to the background. Simulator only.
/// `-pawAppearance dark|light` picks the theme.
enum FixtureLaunch {
    static var isRequested: Bool {
        UserDefaults.standard.bool(forKey: "pawFixture")
    }

    static var colorScheme: ColorScheme? {
        switch UserDefaults.standard.string(forKey: "pawAppearance") {
        case "dark": return .dark
        case "light": return .light
        default: return nil
        }
    }

    private static var faceId: SimFaceId {
        switch UserDefaults.standard.string(forKey: "pawSimFaceId") {
        case "cancel": return .cancel
        case "nomatch": return .noMatch
        case "lockout": return .lockedOut
        case "notenrolled": return .notEnrolled
        default: return .match
        }
    }

    /// Writes the fixture store into a fresh temporary folder and opens it.
    @MainActor
    static func makeModel() throws -> AppModel {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("paw-fixture-\(UUID().uuidString)", isDirectory: true)
        let storeDir = root.appendingPathComponent("store", isDirectory: true)
        let copiesDir = root.appendingPathComponent("window", isDirectory: true)
        try FileManager.default.createDirectory(at: copiesDir, withIntermediateDirectories: true)
        try writeFixtureStore(dir: storeDir.path)

        let store = try PassStore(dir: storeDir.path)
        let keystore = SimKeystore()
        keystore.setFaceId(answer: faceId)
        let clock = SimClock(wallMs: Int64(Date().timeIntervalSince1970 * 1000))
        // One clock for the copies and the key port, so moving it on ends
        // copies and, in the background, the held PIN.
        let sharedClock = SimClockAdapter(sim: clock)
        let copies = CopyCache(
            dir: copiesDir.path, keystore: SimKeystoreAdapter(sim: keystore), clock: sharedClock)
        let key = SimulatedKeyHolder.shared
        let away = UserDefaults.standard.integer(forKey: "pawSimAwayMs")
        return AppModel(
            store: store, copies: copies, clock: sharedClock,
            connect: { try KeyConnection.openForSession() },
            simulator: SimulatorControls(
                advanceClock: { clock.advanceMs(ms: $0) },
                plugIn: { key.key.plugIn() },
                startsUnplugged: key.startsUnplugged,
                awayMs: UInt64(max(away, 0))))
    }
}

/// The core's simulated keystore and clock do not reach Swift as the
/// `DeviceKeystore` and `Clock` traits they implement in Rust, so these
/// pass the calls through.
private final class SimKeystoreAdapter: DeviceKeystore {
    let sim: SimKeystore

    init(sim: SimKeystore) {
        self.sim = sim
    }

    func seal(secret: Data) throws -> Data { try sim.seal(secret: secret) }
    func unseal(sealed: Data, reason: String) throws -> Data { try sim.unseal(sealed: sealed, reason: reason) }
    func indexKey() throws -> Data { try sim.indexKey() }
    func reset() throws { try sim.reset() }
}

private final class SimClockAdapter: Clock {
    let sim: SimClock

    init(sim: SimClock) {
        self.sim = sim
    }

    func wallMs() -> Int64 { sim.wallMs() }
    func monotonicMs() -> Int64 { sim.monotonicMs() }
    func bootId() -> String { sim.bootId() }
}

/// Builds the fixture model once the first frame is up, and says so
/// inline if it cannot.
struct FixtureRoot: View {
    @State private var model: AppModel?
    @State private var problem: String?

    var body: some View {
        Group {
            if let model {
                RootView(model: model)
            } else if let problem {
                ErrorRow(kind: .error, problem: "The test store didn't open.", nextStep: problem)
                    .padding(PawMetrics.Spacing.l)
            } else {
                PawColor.bg.ignoresSafeArea()
            }
        }
        .preferredColorScheme(FixtureLaunch.colorScheme)
        .onAppear {
            guard model == nil, problem == nil else { return }
            do {
                model = try FixtureLaunch.makeModel()
            } catch {
                problem = describe(error)
            }
        }
    }
}
#endif
