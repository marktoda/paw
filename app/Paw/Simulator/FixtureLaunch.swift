import SwiftUI

#if DEBUG && targetEnvironment(simulator)
/// `-pawFixture 1`: the app on a made-up store, with the simulated key
/// (`-pawSimPin`, `-pawSimTouch`, `-pawSimTouchTimeoutMs`, `-pawSimPinTries`,
/// `-pawSimUnplugged`), a simulated
/// Face ID (`-pawSimFaceId ok|cancel|nomatch|lockout|notenrolled`) and a
/// simulated clock that the settings sheet can move on, and that moves on
/// by `-pawSimAwayMs` each time paw goes to the background. Simulator only.
/// `-pawAppearance dark|light` picks the theme. The store can be written
/// (`WritableFixture`, `-pawSimRecipients changed|missing`).
enum FixtureLaunch {
    static var isRequested: Bool {
        LaunchArgument.fixture.bool
    }

    private static var faceId: SimFaceId {
        switch LaunchArgument.simFaceId.string {
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
        // Set up through the simulated git host so it can be written; the
        // plain files, read-only, if that fails (Home then says why).
        let writable = Result { try WritableFixture.prepare(root: root) }
        let storePath: String
        if case .success(let prepared) = writable {
            storePath = prepared.repoDir
        } else {
            try writeFixtureStore(dir: storeDir.path)
            storePath = storeDir.path
        }

        let keystore = SimKeystore()
        keystore.setFaceId(answer: faceId)
        let clock = SimClock(wallMs: Int64(Date().timeIntervalSince1970 * 1000))
        // One clock for the copies and the key port, so moving it on ends
        // copies and, in the background, the held PIN.
        let sharedClock = clock.asClock()
        let copies = CopyCache(dir: copiesDir.path, keystore: keystore.asKeystore(), clock: sharedClock)
        // The key log goes into the fixture's own folder, so Settings ›
        // Key diagnostics shows it; set before the key is first read.
        let keyPort = KeyPort(clock: sharedClock)
        keyPort.setKeyLog(stateDir: root.path, process: "app")
        let session = try StoreSession(StoreSession.Parts(
            repoDir: storePath, copies: copies, clock: sharedClock, keyPort: keyPort,
            connect: { try KeyConnection.open() },
            simulator: .simulated(clock: clock, keyPort: keyPort),
            writing: try? writable.get().writing, gitHub: nil, setUp: nil))
        return AppModel(session: session, notice: WritableFixture.notice(writable))
    }
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
        .preferredColorScheme(LaunchArgument.colorScheme)
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
