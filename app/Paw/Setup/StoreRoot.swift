import Observation
import SwiftUI
import UIKit

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
    /// The protection classes of the store's files, for the simulator's
    /// Settings readback.
    let fileProtection: @MainActor () -> String
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
        /// The store on this iPhone could not be opened: the raw error for
        /// "Details", and the changes that setting up again would lose.
        case failed(detail: String, unpushed: Int)
        /// The store could not be read while the iPhone is locked: iOS
        /// launched paw in the background, or the phone locked mid-open.
        /// Nothing is wrong; it opens again once the phone is unlocked.
        case waitingForUnlock
    }

    private(set) var phase: Phase = .failed(detail: "", unpushed: 0)
    /// Whether the store paw kept before the App Group is still on this
    /// iPhone (`LegacyStore`): "present", "gone", or "unknown" until looked.
    private(set) var legacyFolder = "unknown"
    let location: StoreLocation
    let keyPort: KeyPort
    /// The GitHub sign-in, if this build has one (the simulator only with
    /// `-pawSimGitHub`).
    let gitHub: GitHubAccess?
    private let clock: Clock
    private let connect: AppModel.Connect = { try KeyConnection.open() }
    #if targetEnvironment(simulator)
    private let simulatedClock: SimClock
    let keystore = SimKeystore()
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
        // The app's half of the key log; the AutoFill extension starts its
        // own in the same state directory.
        keyPort.setKeyLog(stateDir: location.stateDir.path, process: "app")
        gitHub = GitHubRefresh.access(stateDir: location.stateDir)
        route()
        // The model lives as long as the app, so the observer is never removed.
        NotificationCenter.default.addObserver(
            forName: UIApplication.protectedDataDidBecomeAvailableNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.retryAfterUnlock() }
        }
        #if DEBUG && targetEnvironment(simulator)
        if LegacyStore.plantsInstead {
            LegacyStore.plant { [weak self] left in self?.legacyFolder = left ? "present" : "gone" }
            return
        }
        #endif
        removeLegacy()
    }

    private func removeLegacy() {
        LegacyStore.remove { [weak self] left in self?.legacyFolder = left ? "present" : "gone" }
    }

    /// Opens the store again if it could not be read before: after the
    /// phone is unlocked, or when paw comes to the front.
    func retryAfterUnlock() {
        switch phase {
        case .failed, .waitingForUnlock: route()
        case .setup, .home: break
        }
    }

    /// Setup if it is not done, else Home.
    func route() {
        gitHub?.reload()
        do {
            #if DEBUG && targetEnvironment(simulator)
            if SimLockedLaunch.isLocked { throw SimLockedLaunch.Unreadable() }
            #endif
            let setup = try Setup.open(stateDir: location.stateDir.path)
            if case .done = setup.state() {
                openHome()
            } else {
                phase = .setup(makeSetup(setup))
            }
        } catch {
            fail(error)
        }
    }

    private func fail(_ error: Error) {
        // While the phone is locked the protected files can't be read, so
        // the failure says nothing about the store: wait for the unlock.
        guard ProtectedData.isAvailable else {
            phase = .waitingForUnlock
            return
        }
        let unpushed = (try? StoreSync.open(stateDir: location.stateDir.path).unpushed().count) ?? 0
        phase = .failed(detail: String(describing: error), unpushed: unpushed)
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
            // Setup wrote the pinned keys, the enrolment and the known hosts.
            location.protect()
            let store = try PassStore(dir: location.repoDir.path)
            let sync = try? StoreSync.open(stateDir: location.stateDir.path)
            #if targetEnvironment(simulator)
            let copies: CopyCache? = CopyCache(
                dir: location.copiesDir.path, keystore: keystore.asKeystore(), clock: clock)
            let held = SimulatedKeyHolder.shared
            let simulatedClock = simulatedClock
            let away = LaunchArguments.int("pawSimAwayMs") ?? 0
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
            model.edits?.syncing?.gitHub = gitHub
            let controls = StoreControls(
                address: sync?.address() ?? "",
                hostFingerprints: (try? sync?.hostFingerprints()) ?? [],
                unpushed: (try? sync?.unpushed().count) ?? 0,
                copiesAvailable: copies != nil,
                remove: { [weak self] in self?.removeStore() },
                fileProtection: { [location] in location.protectionSummary() })
            model.publishIdentities = { CredentialIdentities.publish($0) }
            let codes = OneTimeCodeEntries.open(stateDir: location.stateDir.path)
            model.noteCode = { CredentialIdentities.noteOpened($0, path: $1, codes: codes) }
            CredentialIdentities.publish(store)
            phase = .home(model, controls)
        } catch {
            fail(error)
        }
    }

    /// Settings › Remove: ends every Face ID copy and forgets the PIN,
    /// deletes the store and everything setup kept, and starts setup again.
    private func removeStore() -> String? {
        BackgroundTime.run("Remove store") { removeStoreNow() }
    }

    private func removeStoreNow() -> String? {
        if case .home(let model, _) = phase {
            model.lockNow()
        }
        // Whatever Lock now managed, and even with no Face ID copies on this
        // iPhone: the seal key, the index key and the enrolment marker go.
        let keychain = wipeKeychain()
        Clipboard.clearIfOurs()
        let files = wipeAndRestart()
        removeLegacy()
        return [keychain, files].compactMap { $0 }.first
    }

    /// Deletes the Face ID copies' keychain items. Returns why not, if not.
    private func wipeKeychain() -> String? {
        do {
            #if targetEnvironment(simulator)
            try keystore.reset()
            #else
            try EnclaveKeystore().resetDetailed()
            #endif
            return nil
        } catch {
            return "paw couldn't delete the Face ID keys from the keychain: \(describe(error))"
        }
    }

    /// "Start setup again" after the host's identity changed, or after the
    /// store could not be opened.
    func startOver() {
        _ = wipeKeychain()
        if let problem = wipeAndRestart() {
            phase = .failed(detail: problem, unpushed: 0)
        }
    }

    private func wipeAndRestart() -> String? {
        // Drops the PIN at once; the card is de-verified on the session's
        // own thread, never here on the main thread.
        keyPort.session()?.forgetPin()
        keyPort.setEnrolledKey(key: nil)
        CredentialIdentities.clear()
        // Gone with the folder below too; cleared first in case that fails.
        try? SitePairings.open(stateDir: location.stateDir.path).clearPairings(entry: nil)
        try? OneTimeCodeEntries.open(stateDir: location.stateDir.path).clear()
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
/// Home, and says so inline if the store can't be read, with a way out.
struct StoreRoot: View {
    @State private var root: StoreRootModel?
    @State private var problem: String?
    /// paw's folder could not be read while the phone was locked.
    @State private var waitingForUnlock = false
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        Group {
            if let root {
                StoreRootContent(root: root)
            } else if waitingForUnlock {
                WaitingForUnlockView()
            } else if let problem {
                StoreProblemView(
                    detail: problem, unpushed: Self.unpushedOnDisk(), retry: { build() },
                    startOver: { removeEverything() })
            } else {
                PawColor.bg.ignoresSafeArea()
            }
        }
        .preferredColorScheme(Self.appearance)
        .onAppear {
            guard root == nil, problem == nil else { return }
            build()
        }
        .onReceive(NotificationCenter.default.publisher(for: UIApplication.protectedDataDidBecomeAvailableNotification)) { _ in
            if root == nil { build() }
        }
        .onChange(of: scenePhase) { _, phase in
            guard phase == .active else { return }
            if root == nil, problem != nil || waitingForUnlock {
                build()
            } else {
                root?.retryAfterUnlock()
            }
        }
    }

    private func build() {
        do {
            root = try StoreRootModel()
            problem = nil
            waitingForUnlock = false
        } catch {
            if ProtectedData.isAvailable {
                problem = String(describing: error)
                waitingForUnlock = false
            } else {
                problem = nil
                waitingForUnlock = true
            }
        }
    }

    /// "Remove the store" when even opening paw's folder failed: what the
    /// normal remove deletes, as far as it can without the store open.
    private func removeEverything() {
        #if !targetEnvironment(simulator)
        DispatchQueue.global(qos: .utility).async { try? EnclaveKeystore().resetDetailed() }
        #endif
        CredentialIdentities.clear()
        Clipboard.clearIfOurs()
        LegacyStore.remove()
        do {
            try StoreLocation.standard().removeAll()
            build()
        } catch {
            problem = String(describing: error)
        }
    }

    private static func unpushedOnDisk() -> Int {
        guard let location = try? StoreLocation.standard() else { return 0 }
        return (try? StoreSync.open(stateDir: location.stateDir.path).unpushed().count) ?? 0
    }

    private static var appearance: ColorScheme? {
        #if DEBUG && targetEnvironment(simulator)
        FixtureLaunch.colorScheme
        #else
        nil
        #endif
    }
}

private struct StoreRootContent: View {
    let root: StoreRootModel

    var body: some View {
        Group {
            switch root.phase {
            case .setup(let model):
                SetupFlowView(model: model)
            case .home(let model, let controls):
                RootView(model: model)
                    .environment(\.storeControls, controls)
            case .failed(let detail, let unpushed):
                StoreProblemView(
                    detail: detail, unpushed: unpushed, retry: { root.route() }, startOver: { root.startOver() })
            case .waitingForUnlock:
                WaitingForUnlockView()
            }
        }
        .environment(\.gitHubAccess, root.gitHub)
        #if DEBUG && targetEnvironment(simulator)
        .background(alignment: .topLeading) {
            // For the UI tests: what is left of a store after it was removed.
            let state = root.debugState()
            Text(verbatim: state)
                .font(.caption2)
                .frame(width: 1, height: 1)
                .opacity(0.02)
                .accessibilityLabel("Store state")
                .accessibilityValue(state)
                .accessibilityIdentifier("sim-store-state")
        }
        #endif
    }
}

/// Whether files protected with `NSFileProtectionComplete` can be read now.
enum ProtectedData {
    @MainActor static var isAvailable: Bool {
        #if DEBUG && targetEnvironment(simulator)
        if SimLockedLaunch.isLocked { return false }
        #endif
        return UIApplication.shared.isProtectedDataAvailable
    }

    /// Whether `detail` reads like a file paw was not allowed to read, as
    /// protected files are while the phone is locked. Such an error never
    /// offers to delete the store: the store is probably fine.
    static func mayBeLocked(_ detail: String) -> Bool {
        let signs = [
            "Operation not permitted", "os error 1)", "Permission denied", "os error 13)",
            "NSFileReadNoPermissionError", "NSFileWriteNoPermissionError", "Code=257", "Code=513",
        ]
        return signs.contains { detail.localizedCaseInsensitiveContains($0) }
    }
}

/// Nothing to show yet: the phone is locked and the store unreadable. paw
/// opens it as soon as the phone is unlocked.
private struct WaitingForUnlockView: View {
    var body: some View {
        PawColor.bg
            .ignoresSafeArea()
            .accessibilityIdentifier("store-waiting-for-unlock")
            #if DEBUG && targetEnvironment(simulator)
            .overlay(alignment: .bottom) {
                SimulatorStrip(actions: [
                    SimulatorAction(title: "unlock the iPhone", identifier: "sim-unlock") { SimLockedLaunch.unlock() }
                ])
                .padding(PawMetrics.Spacing.l)
            }
            #endif
    }
}

#if DEBUG && targetEnvironment(simulator)
/// `-pawSimLockedLaunch 1`: launch as iOS does in the background while the
/// phone is locked, with the store's files unreadable, until the debug
/// "unlock" control posts the system's own unlock notification.
enum SimLockedLaunch {
    struct Unreadable: Error, CustomStringConvertible {
        var description: String { "Operation not permitted (os error 1) (simulated lock)" }
    }

    @MainActor static var isLocked = LaunchArguments.bool("pawSimLockedLaunch")

    @MainActor static func unlock() {
        isLocked = false
        NotificationCenter.default.post(name: UIApplication.protectedDataDidBecomeAvailableNotification, object: nil)
    }
}

extension StoreRootModel {
    /// "copies 0 · keychain resets 1 · pin none · pasteboard empty · legacy gone".
    func debugState() -> String {
        let copies = (try? FileManager.default.contentsOfDirectory(atPath: location.copiesDir.path).count) ?? 0
        let pin = keyPort.session()?.holdsPin() == true ? "held" : "none"
        let pasteboard = UIPasteboard.general.hasStrings ? "full" : "empty"
        return "copies \(copies) · keychain resets \(keystore.resetCount()) · pin \(pin) · pasteboard \(pasteboard)"
            + " · legacy \(legacyFolder)"
    }
}
#endif

/// The store could not be opened. Say what happened in words, keep the
/// library's text under Details, and offer the ways out: try again, or set
/// up again after an inline confirmation that says what would be lost.
private struct StoreProblemView: View {
    let detail: String
    /// Changes saved on this iPhone and not sent: lost by setting up again.
    let unpushed: Int
    let retry: () -> Void
    let startOver: () -> Void

    @State private var confirming = false

    /// Another part of paw held the store for too long. Nothing is broken, so
    /// this never offers to delete anything: it waits and tries again.
    private var busy: Bool { isStoreBusy(message: detail) }

    var body: some View {
        if busy {
            VStack(spacing: PawMetrics.Spacing.l) {
                ProgressView()
                Text("The store is busy. Trying again\u{2026}")
                    .font(PawFont.subheadline)
                    .foregroundStyle(PawColor.labelSecondary)
                Button("Try again", action: retry)
                    .buttonStyle(.pawSecondary)
                    .accessibilityIdentifier("store-retry")
            }
            .padding(PawMetrics.Spacing.l)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(PawColor.bg.ignoresSafeArea())
            .accessibilityIdentifier("store-busy")
            .task {
                // Leaving this screen cancels the task, which ends the loop.
                while !Task.isCancelled {
                    try? await Task.sleep(nanoseconds: 2_000_000_000)
                    if Task.isCancelled { break }
                    retry()
                }
            }
        } else {
            problemBody
        }
    }

    private var problemBody: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.l) {
                ErrorRow(
                    kind: .error, problem: Device.words("Couldn't open the store on this iPhone"),
                    nextStep: ProtectedData.mayBeLocked(detail)
                        ? Device.words("Unlock your iPhone, then try again.")
                        : "Try again, or set it up again from your server.",
                    isBanner: true)
                if !detail.isEmpty {
                    Text("Details: \(detail)")
                        .font(PawFont.footnote.monospaced())
                        .foregroundStyle(PawColor.labelSecondary)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityIdentifier("store-problem-detail")
                }
                Button("Try again", action: retry)
                    .buttonStyle(.pawPrimary)
                    .accessibilityIdentifier("store-retry")
                // An error that may only mean the phone was locked never
                // offers to delete the store: it is probably fine.
                if !ProtectedData.mayBeLocked(detail) {
                    startOverControls
                }
            }
            .padding(PawMetrics.Spacing.l)
        }
        .frame(maxHeight: .infinity)
        .background(PawColor.bg.ignoresSafeArea())
        .accessibilityIdentifier("store-problem")
    }

    @ViewBuilder
    private var startOverControls: some View {
        if confirming {
            Text(warning)
                .font(PawFont.subheadline)
                .foregroundStyle(PawColor.label)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityIdentifier("store-start-over-warning")
            Button("Delete and set up again", action: startOver)
                .buttonStyle(.pawSecondary)
                .foregroundStyle(PawColor.danger)
                .accessibilityIdentifier("store-start-over-confirm")
            Button("Keep it") { confirming = false }
                .buttonStyle(.pawSecondary)
                .accessibilityIdentifier("store-start-over-cancel")
        } else {
            Button("Set up again\u{2026}") { confirming = true }
                .buttonStyle(.pawSecondary)
                .accessibilityIdentifier("store-start-over")
        }
    }

    private var warning: String {
        var text = Device.words("Deletes the store, its Face ID copies and your key's record from this iPhone. ")
            + "The store on the server is not touched."
        if unpushed == 1 {
            text += " 1 change not sent yet will be lost."
        } else if unpushed > 1 {
            text += " \(unpushed) changes not sent yet will be lost."
        }
        return text
    }
}
