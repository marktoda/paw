import Observation
import SwiftUI
import UIKit

/// The app outside the simulator's fixture: setup until a store is set up,
/// then Home on the cloned store. One key port serves both, so the PIN
/// typed during setup is still held on Home.
@MainActor @Observable
final class StoreRootModel {
    enum Phase {
        case setup(SetupModel)
        case home(AppModel)
        /// The store on this iPhone could not be opened: the raw error for
        /// "Details", the changes that setting up again would lose (`nil`
        /// when they can't be counted), whether it was only busy, and
        /// whether the iPhone may only have been locked.
        case failed(detail: String, unpushed: Int?, busy: Bool, mayBeLocked: Bool)
        /// The store could not be read while the iPhone is locked: iOS
        /// launched paw in the background, or the phone locked mid-open.
        /// Nothing is wrong; it opens again once the phone is unlocked.
        case waitingForUnlock
    }

    /// The store's folders and the key port that keeps its key log there:
    /// built again on each try until the folders can be made.
    @MainActor
    private struct Base {
        let location: StoreLocation
        let keys: StoreKeys
        var keyPort: KeyPort { keys.keyPort }

        init(_ platform: Platform) throws {
            let location = try StoreLocation.standard()
            try location.prepare()
            self.location = location
            keys = StoreKeys(platform, at: location)
        }
    }

    private(set) var phase: Phase = .waitingForUnlock
    private let platform: Platform
    private var base: Base?
    /// The GitHub sign-in, if this build has one (the simulator only with
    /// `-pawSimGitHub`).
    private(set) var gitHub: GitHubAccess?

    init(platform: Platform) {
        self.platform = platform
        route()
        // The model lives as long as the app, so the observer is never removed.
        NotificationCenter.default.addObserver(
            forName: UIApplication.protectedDataDidBecomeAvailableNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.retryAfterUnlock() }
        }
        if platform.removesLegacyStore {
            LegacyStore.remove()
        }
    }

    /// Opens the store again if it could not be read before: after the
    /// phone is unlocked, or when paw comes to the front.
    func retryAfterUnlock() {
        switch phase {
        case .failed, .waitingForUnlock: route()
        case .setup, .home: break
        }
    }

    func route() {
        guard let base = base ?? makeBase() else { return }
        gitHub?.reload()
        do {
            if let refused = platform.protectedDataRefusal() { throw refused }
            let setup = try Setup.open(stateDir: base.location.stateDir.path)
            if case .done = setup.state() {
                openHome()
            } else {
                phase = .setup(makeSetup(setup, base))
            }
        } catch {
            fail(error)
        }
    }

    private func makeBase() -> Base? {
        do {
            let base = try Base(platform)
            self.base = base
            let location = base.location
            gitHub = GitHubSignInSource.access(stateDir: location.stateDir)
            return base
        } catch {
            fail(error)
            return nil
        }
    }

    /// Where the store is, even when its folders could not be made.
    private var reachableLocation: StoreLocation? {
        base?.location ?? (try? StoreLocation.standard())
    }

    private func fail(_ error: Error) {
        // While the phone is locked the protected files can't be read, so
        // the failure says nothing about the store: wait for the unlock.
        guard platform.isProtectedDataAvailable else {
            phase = .waitingForUnlock
            return
        }
        let stateDir = reachableLocation?.stateDir
        phase = .failed(
            detail: String(describing: error), unpushed: Self.unpushed(stateDir: stateDir), busy: isStoreBusy(error),
            mayBeLocked: ProtectedData.mayBeLocked(error, stateDir: stateDir))
    }

    /// The changes saved in `stateDir` and not sent; `nil` when the queue
    /// can't be read, so nobody is told nothing will be lost.
    static func unpushed(stateDir: URL?) -> Int? {
        guard let stateDir else { return nil }
        return try? StoreSync.open(stateDir: stateDir.path).unpushed().count
    }

    private func makeSetup(_ setup: Setup, _ base: Base) -> SetupModel {
        let location = base.location, keyPort = base.keyPort
        return SetupModel(
            setup: setup, repoPath: location.repoDir.path, keyPort: keyPort, keys: platform.keys,
            finished: { [weak self] in self?.openHome() },
            startOver: { [weak self] in self?.startOver() })
    }

    private func openHome() {
        guard let base else { return }
        let location = base.location, stateDir = location.stateDir.path
        do {
            let copies = try base.keys.openEnrolled()
            // Setup wrote the pinned keys, the enrolment and the known hosts.
            location.protect()
            // Setup pinned the key and accepted the recipients: saving
            // works from here with no further step.
            let session = try StoreSession(StoreSession.Parts(
                repoDir: location.repoDir.path, copies: copies, clock: platform.clock, keyPort: base.keyPort,
                keys: platform.keys,
                writing: StoreSession.Writing(
                    writer: StoreWriter.open(stateDir: stateDir), sync: Result { try StoreSync.open(stateDir: stateDir) },
                    stateDir: stateDir),
                gitHub: gitHub,
                autoFill: AutoFillSuggestions(stateDir: stateDir),
                setUp: SetUpStore(location: location, remove: { [weak self] in self?.removeStore() })))
            phase = .home(AppModel(session: session))
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
        if case .home(let model) = phase {
            model.storeSession.live.announce(.removed)
        }
        let problems = wipeAndRestart()
        if let keychain = problems.keychain {
            return "paw couldn't delete the Face ID keys from the keychain: \(describe(keychain))"
        }
        return problems.files.map { "paw couldn't delete the store: \($0.localizedDescription)" }
    }

    /// "Start setup again" after the host's identity changed, and "Delete
    /// and set up again" when the store could not be opened.
    func startOver() {
        if let files = wipeAndRestart().files {
            let stateDir = reachableLocation?.stateDir
            phase = .failed(
                detail: "paw couldn't delete the store: \(files.localizedDescription)",
                unpushed: Self.unpushed(stateDir: stateDir), busy: false,
                mayBeLocked: ProtectedData.mayBeLocked(files, stateDir: stateDir))
        }
    }

    /// Wipes everything the store left on this iPhone, then setup again.
    /// Without the store's folders only what lies outside them goes.
    private func wipeAndRestart() -> StoreWipe.Problems {
        let location = reachableLocation
        let platform = platform
        var problems = StoreWipe.wipe(location, keyPort: base?.keyPort, resetKeychain: { try platform.resetKeychain() })
        guard problems.files == nil else { return problems }
        do {
            try location?.prepare()
        } catch {
            problems.files = error
            return problems
        }
        route()
        return problems
    }
}

/// Everything paw keeps for a store on this iPhone, in one list: wiped by
/// Settings › Remove, by "Start setup again", and when the store can't even
/// be opened.
@MainActor
enum StoreWipe {
    struct Problems {
        var keychain: Error?
        var files: Error?
    }

    /// The folders go only with a `location`. `keyPort` forgets the PIN
    /// and the enrolled key; `resetKeychain` deletes the Face ID copies'
    /// keychain items (`Platform.resetKeychain`).
    static func wipe(_ location: StoreLocation?, keyPort: KeyPort?, resetKeychain: () throws -> Void) -> Problems {
        var problems = Problems()
        // Drops the PIN at once; the card is de-verified on the session's
        // own thread, never here on the main thread.
        keyPort?.session()?.forgetPin()
        keyPort?.setEnrolledKey(key: nil)
        do {
            try resetKeychain()
        } catch {
            problems.keychain = error
        }
        CredentialIdentities.clear()
        LegacyStore.remove()
        guard let location else { return problems }
        // Gone with the folder below too; cleared first in case that fails.
        try? SitePairings.open(stateDir: location.stateDir.path).clearPairings(entry: nil)
        try? OneTimeCodeEntries.open(stateDir: location.stateDir.path).clear()
        do {
            try location.removeAll()
        } catch {
            problems.files = error
        }
        return problems
    }
}

/// Builds the root model once the first frame is up, then shows setup or
/// Home, and says so inline if the store can't be read, with a way out.
struct StoreRoot: View {
    let platform: Platform
    @State private var root: StoreRootModel?
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        Group {
            if let root {
                StoreRootContent(root: root)
            } else {
                PawColor.bg.ignoresSafeArea()
            }
        }
        .preferredColorScheme(Self.appearance)
        .onAppear {
            if root == nil { root = StoreRootModel(platform: platform) }
        }
        .onChange(of: scenePhase) { _, phase in
            guard phase == .active else { return }
            root?.retryAfterUnlock()
        }
    }

    private static var appearance: ColorScheme? {
        #if DEBUG && targetEnvironment(simulator)
        LaunchArgument.colorScheme
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
                SetupFlowView(model: model, gitHub: root.gitHub)
            case .home(let model):
                RootView(model: model)
            case .failed(let detail, let unpushed, let busy, let mayBeLocked):
                StoreProblemView(
                    detail: detail, unpushed: unpushed, busy: busy, mayBeLocked: mayBeLocked,
                    retry: { root.route() }, startOver: { root.startOver() })
            case .waitingForUnlock:
                WaitingForUnlockView()
            }
        }
        #if DEBUG && targetEnvironment(simulator)
        .background(alignment: .topLeading) {
            SimStoreState(phase: root.phase)
        }
        #endif
    }
}

extension Platform {
    /// Whether files protected with `NSFileProtectionComplete` can be read
    /// now: not while the iPhone is locked.
    @MainActor var isProtectedDataAvailable: Bool {
        protectedDataRefusal() == nil && UIApplication.shared.isProtectedDataAvailable
    }
}

enum ProtectedData {
    /// Whether `error` may only mean that the system refused paw's files,
    /// as iOS does for protected files while the phone is locked: the
    /// error says so, or the store's files in `stateDir` are refused now
    /// (the core's errors carry no such kind). Such an error never offers
    /// to delete the store: the store is probably fine.
    static func mayBeLocked(_ error: Error, stateDir: URL?) -> Bool {
        if let error = error as? CocoaError,
           error.code == .fileReadNoPermission || error.code == .fileWriteNoPermission {
            return true
        }
        if let error = error as? POSIXError, error.code == .EPERM || error.code == .EACCES {
            return true
        }
        return stateDir.map { storeFilesRefused(stateDir: $0.path) } ?? false
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

/// The store could not be opened. Say what happened in words, keep the
/// library's text under Details, and offer the ways out: try again, or set
/// up again after an inline confirmation that says what would be lost.
private struct StoreProblemView: View {
    let detail: String
    /// Changes saved on this iPhone and not sent: lost by setting up again.
    /// `nil` when they couldn't be counted.
    let unpushed: Int?
    /// Another part of paw held the store for too long. Nothing is broken, so
    /// this never offers to delete anything: it waits and tries again.
    let busy: Bool
    /// The phone may only have been locked. The store is probably fine, so
    /// this never offers to delete it either.
    let mayBeLocked: Bool
    let retry: () -> Void
    let startOver: () -> Void

    @State private var confirming = false

    var body: some View {
        if busy {
            VStack(spacing: PawMetrics.Spacing.l) {
                ProgressView()
                Text(StoreBusyWords.line)
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
                    kind: .error, problem: "Couldn't open the store on this iPhone",
                    nextStep: mayBeLocked
                        ? "Unlock your iPhone, then try again."
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
                if !mayBeLocked {
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
        removeStoreWarning(unpushed: unpushed)
    }
}
