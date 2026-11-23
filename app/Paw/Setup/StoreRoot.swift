import Observation
import SwiftUI

/// What Settings shows and does about the store: its address and host,
/// whether Face ID copies exist on this iPhone, and removing it.
struct StoreControls {
    let address: String
    let hostFingerprints: [String]
    /// Passwords not yet pushed, lost if the store is removed.
    let unpushed: Int
    /// `false` on a device until the Secure Enclave keystore exists.
    let copiesAvailable: Bool
    /// Removes the store from this iPhone and returns to setup. Returns
    /// why it could not, if it could not.
    let remove: @MainActor () -> String?
}

extension EnvironmentValues {
    @Entry var storeControls: StoreControls? = nil
}

/// The app outside the simulator's fixture: setup until a store is set up,
/// then Home on the cloned store. One key port serves both, so the PIN
/// typed during setup is still held on Home.
@MainActor @Observable
final class StoreRootModel {
    enum Phase {
        case setup(SetupModel)
        case home(AppModel, StoreControls)
        case failed(String)
    }

    private(set) var phase: Phase = .failed("")
    let location: StoreLocation
    let keyPort: KeyPort
    private let clock: Clock
    private let connect: AppModel.Connect = { try KeyConnection.openForSession() }
    #if targetEnvironment(simulator)
    private let simulatedClock: SimClock
    private let keystore = SimKeystore()
    #endif

    init() throws {
        let location = try StoreLocation.standard()
        #if targetEnvironment(simulator)
        if SetupLaunch.isRequested, !SetupLaunch.keepsState {
            try location.removeAll()
        }
        let simulatedClock = SimClock(wallMs: Int64(Date().timeIntervalSince1970 * 1000))
        let clock = simulatedClock.asClock()
        self.simulatedClock = simulatedClock
        #else
        let clock: Clock = SystemClock()
        #endif
        try location.prepare()
        self.location = location
        self.clock = clock
        keyPort = KeyPort(clock: clock)
        route()
    }

    /// Setup if it is not done, else Home.
    func route() {
        do {
            let setup = try Setup.open(stateDir: location.stateDir.path)
            if case .done = setup.state() {
                openHome()
            } else {
                phase = .setup(makeSetup(setup))
            }
        } catch {
            phase = .failed(String(describing: error))
        }
    }

    private func makeSetup(_ setup: Setup) -> SetupModel {
        #if targetEnvironment(simulator)
        let simulator: SetupSimulator? = SimulatedHostHolder.shared.setupSimulator()
        #else
        let simulator: SetupSimulator? = nil
        #endif
        return SetupModel(
            setup: setup, repoPath: location.repoDir.path, keyPort: keyPort, connect: connect,
            simulator: simulator,
            finished: { [weak self] in self?.openHome() },
            startOver: { [weak self] in self?.startOver() })
    }

    /// Home on the cloned store, with the key enrolled at setup.
    private func openHome() {
        do {
            _ = try keyPort.loadEnrolledKey(stateDir: location.stateDir.path)
            let store = try PassStore(dir: location.repoDir.path)
            let sync = try? StoreSync.open(stateDir: location.stateDir.path)
            #if targetEnvironment(simulator)
            let copies: CopyCache? = CopyCache(
                dir: location.copiesDir.path, keystore: keystore.asKeystore(), clock: clock)
            let held = SimulatedKeyHolder.shared
            let simulatedClock = simulatedClock
            let away = UserDefaults.standard.integer(forKey: "pawSimAwayMs")
            let simulator: SimulatorControls? = SimulatorControls(
                advanceClock: { simulatedClock.advanceMs(ms: $0) },
                plugIn: { held.key.plugIn() },
                startsUnplugged: held.startsUnplugged && keyPort.session() == nil,
                awayMs: UInt64(max(away, 0)))
            #else
            // Face ID copies in the Secure Enclave; `nil` where it can't be
            // used, and then every open uses the key (Settings says so).
            let copies = PlatformCopies.make(dir: location.copiesDir.path)
            let simulator: SimulatorControls? = nil
            #endif
            let model = AppModel(
                store: store, copies: copies, clock: clock, connect: connect, simulator: simulator,
                keyPort: keyPort)
            // Setup pinned the key and accepted the recipients: saving
            // works from here with no further step.
            model.edits = StoreEdits(
                writer: StoreWriter.open(stateDir: location.stateDir.path), sync: sync,
                repoDir: location.repoDir.path, sendingNote: nil)
            let controls = StoreControls(
                address: sync?.address() ?? "",
                hostFingerprints: (try? sync?.hostFingerprints()) ?? [],
                unpushed: (try? sync?.unpushed().count) ?? 0,
                copiesAvailable: copies != nil,
                remove: { [weak self] in self?.removeStore() })
            phase = .home(model, controls)
        } catch {
            phase = .failed(String(describing: error))
        }
    }

    /// Settings › Remove: ends every Face ID copy and forgets the PIN,
    /// deletes the store and everything setup kept, and starts setup again.
    private func removeStore() -> String? {
        if case .home(let model, _) = phase {
            model.lockNow()
        }
        return wipeAndRestart()
    }

    /// "Start setup again" after the host's identity changed, or after the
    /// store could not be opened.
    func startOver() {
        if let problem = wipeAndRestart() {
            phase = .failed(problem)
        }
    }

    private func wipeAndRestart() -> String? {
        keyPort.session()?.forgetPin()
        keyPort.setEnrolledKey(key: nil)
        do {
            try location.removeAll()
            try location.prepare()
        } catch {
            return "paw couldn't delete the store: \(error.localizedDescription)"
        }
        route()
        return nil
    }
}

/// Builds the root model once the first frame is up, then shows setup or
/// Home, and says so inline if the store can't be read.
struct StoreRoot: View {
    @State private var root: StoreRootModel?
    @State private var problem: String?

    var body: some View {
        Group {
            if let root {
                StoreRootContent(root: root)
            } else if let problem {
                StoreProblemView(problem: problem, startOver: nil)
            } else {
                PawColor.bg.ignoresSafeArea()
            }
        }
        .preferredColorScheme(Self.appearance)
        .onAppear {
            guard root == nil, problem == nil else { return }
            do {
                root = try StoreRootModel()
            } catch {
                problem = String(describing: error)
            }
        }
    }

    private static var appearance: ColorScheme? {
        #if targetEnvironment(simulator)
        FixtureLaunch.colorScheme
        #else
        nil
        #endif
    }
}

private struct StoreRootContent: View {
    let root: StoreRootModel

    var body: some View {
        switch root.phase {
        case .setup(let model):
            SetupFlowView(model: model)
        case .home(let model, let controls):
            RootView(model: model)
                .environment(\.storeControls, controls)
        case .failed(let problem):
            StoreProblemView(problem: problem) { root.startOver() }
        }
    }
}

/// The store could not be opened. Nothing else is possible, so say what
/// happened and offer the one way out.
private struct StoreProblemView: View {
    let problem: String
    let startOver: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.l) {
            ErrorRow(
                kind: .error, problem: "paw couldn't open the store on this iPhone",
                nextStep: "Quit paw and open it again. If that doesn't help, set it up again. \(problem)",
                isBanner: true)
            if let startOver {
                Button("Set up again", action: startOver)
                    .buttonStyle(.pawSecondary)
                    .accessibilityIdentifier("store-start-over")
            }
        }
        .padding(PawMetrics.Spacing.l)
        .frame(maxHeight: .infinity)
        .background(PawColor.bg.ignoresSafeArea())
        .accessibilityIdentifier("store-problem")
    }
}
