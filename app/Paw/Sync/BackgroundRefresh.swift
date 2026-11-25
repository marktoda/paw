import BackgroundTasks
import Foundation
import UIKit

/// The last background check: when, and how it went (never an entry name
/// or an error's text). Kept in the app's own defaults, which can be read
/// and written while the iPhone is locked; the store can't (it is
/// `complete`-protected).
struct BackgroundRun: Equatable {
    /// How a run went, kept by name and said by the status page.
    enum Outcome: String {
        case locked, noStore, notSignedIn, notLinked, upToDate, broughtIn, paused, signedOut, offline, outOfTime,
             busy, failed

        /// As Sync says it, in the iPhone's words.
        var words: String {
            switch self {
            case .locked: "iPhone was locked"
            case .noStore: "No store yet"
            case .notSignedIn: "Not signed in to GitHub"
            case .notLinked: "No store chosen"
            case .upToDate: "Up to date"
            case .broughtIn: "Brought in changes"
            case .paused: "Paused"
            case .signedOut: "GitHub sign-in ended"
            case .offline: "Couldn't reach GitHub"
            case .outOfTime: "Ran out of time"
            case .busy: "Store was busy"
            case .failed: "Failed"
            }
        }

        /// Ended as it should (for `setTaskCompleted`).
        var success: Bool {
            switch self {
            case .offline, .outOfTime, .busy, .failed: false
            case .locked, .noStore, .notSignedIn, .notLinked, .upToDate, .broughtIn, .paused, .signedOut: true
            }
        }
    }

    let at: Date
    let outcome: Outcome
    /// Changes came in that the list has not shown yet.
    let changed: Bool
    /// Something only the person can decide was found and left alone.
    let needsLook: Bool

    var success: Bool {
        outcome.success
    }
}

/// The silent pull in the background (design 2.9): a `BGAppRefreshTask`
/// that fetches with the GitHub sign-in, with no key, no PIN and no Face
/// ID, and integrates only what needs no decision (the core's
/// `background_refresh`). Anything that needs the person is left for the
/// next open, where a check with the sign-in shows it as usual.
///
/// Registered before launch ends (`PawAppDelegate`); scheduled each time
/// paw goes to the background and after each run, at the earliest an hour
/// later. iOS decides when it runs, if at all.
enum BackgroundRefresh {
    static let identifier = "com.marktoda.paw.refresh"
    /// iOS gives a refresh task about 30 seconds; the core stops by this.
    static let budgetSeconds: UInt32 = 25
    static let interval: TimeInterval = 3_600
    /// Posted on the main queue when a run ends (paw may be on screen).
    static let finished = Notification.Name("pawBackgroundRefreshFinished")

    private static let queue = DispatchQueue(label: "paw-background-refresh", qos: .utility)
    private static let atKey = "pawBackgroundRunAt"
    private static let resultKey = "pawBackgroundRunResult"
    private static let changedKey = "pawBackgroundRunChanged"
    private static let needsLookKey = "pawBackgroundRunNeedsLook"

    // MARK: Registering and scheduling

    /// Call once, before `didFinishLaunching` returns.
    @MainActor
    static func register() {
        _ = BGTaskScheduler.shared.register(forTaskWithIdentifier: identifier, using: .main) { task in
            MainActor.assumeIsolated {
                guard let task = task as? BGAppRefreshTask else {
                    task.setTaskCompleted(success: false)
                    return
                }
                handle(task)
            }
        }
        _ = NotificationCenter.default.addObserver(
            forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main
        ) { _ in
            MainActor.assumeIsolated {
                enteredBackground()
            }
        }
    }

    /// Asks iOS for the next run. Replaces any request already made.
    static func schedule() {
        let request = BGAppRefreshTaskRequest(identifier: identifier)
        request.earliestBeginDate = Date(timeIntervalSinceNow: interval)
        // Refused in the simulator and when Background App Refresh is off:
        // then there is simply no background check.
        try? BGTaskScheduler.shared.submit(request)
    }

    @MainActor
    private static func enteredBackground() {
        schedule()
        #if DEBUG && targetEnvironment(simulator)
        // UI tests: iOS never runs the task in the simulator, so
        // `-pawSimBackgroundRefresh 1` runs its body on every trip to the
        // background instead.
        if LaunchArguments.bool("pawSimBackgroundRefresh") {
            let time = BackgroundTime.begin("Background refresh")
            start(stop: BackgroundStop()) { _ in time.end() }
        }
        #endif
    }

    @MainActor
    private static func handle(_ task: BGAppRefreshTask) {
        schedule()
        let stop = BackgroundStop()
        // The core stops at its next step or chunk and returns; the task
        // is completed then, never with the store lock still held.
        task.expirationHandler = { stop.stop() }
        let handoff = Handoff(value: task)
        start(stop: stop) { run in
            handoff.value.setTaskCompleted(success: run.success)
        }
    }

    // MARK: One run

    /// Runs the refresh off the main thread, records it, republishes the
    /// AutoFill suggestions if the store changed, then calls `done`.
    @MainActor
    private static func start(stop: BackgroundStop, done: @escaping @MainActor (BackgroundRun) -> Void) {
        // The store is unreadable while the iPhone is locked: skip, and say so.
        let readable = UIApplication.shared.isProtectedDataAvailable
        // The phone may lock mid-run, and then the files go unreadable
        // under the core: stop it at its next step, as iOS's expiry does.
        let locking = NotificationCenter.default.addObserver(
            forName: UIApplication.protectedDataWillBecomeUnavailableNotification, object: nil, queue: .main
        ) { _ in stop.stop() }
        queue.async {
            let run = readable ? runNow(stop: stop) : skipped(.locked)
            let store = run.changed && !stop.isStopped() ? openStore() : nil
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    NotificationCenter.default.removeObserver(locking)
                    record(run)
                    Task { @MainActor in
                        // Stopped (time is up, or the phone locked): end the
                        // task now. The next open publishes the suggestions.
                        if let store, !stop.isStopped() {
                            await CredentialIdentities.publishAndWait(store)
                        }
                        NotificationCenter.default.post(name: finished, object: nil)
                        done(run)
                    }
                }
            }
        }
    }

    /// The refresh itself. Blocks for at most `budgetSeconds`.
    private static func runNow(stop: BackgroundStop) -> BackgroundRun {
        guard let location = try? StoreLocation.standard(), location.isSetUp else {
            return skipped(.noStore)
        }
        guard let core = GitHubRefresh.account(stateDir: location.stateDir.path) else {
            return skipped(.notSignedIn)
        }
        return SignInCheck(core: core).runInBackground(seconds: budgetSeconds, stop: stop, repoDir: location.repoDir.path)
    }

    private static func openStore() -> PassStore? {
        guard let location = try? StoreLocation.standard() else { return nil }
        return try? PassStore(dir: location.repoDir.path)
    }

    private static func skipped(_ outcome: BackgroundRun.Outcome) -> BackgroundRun {
        BackgroundRun(at: Date(), outcome: outcome, changed: false, needsLook: false)
    }

    /// The core's report as a run.
    static func summary(of report: BackgroundRefreshReport, at: Date) -> BackgroundRun {
        let changed = report.integration.map { $0.fastForwarded > 0 || $0.replayed > 0 } ?? false
        // Held-back changes are said on Home already; they stop nothing here.
        let needsLook = report.decisions.contains { decision in
            if case .heldBack = decision { return false }
            return true
        }
        let outcome: BackgroundRun.Outcome
        switch report.outcome {
        case .notSignedIn: outcome = .notSignedIn
        case .notLinked: outcome = .notLinked
        case .upToDate: outcome = .upToDate
        case .updated: outcome = .broughtIn
        case .needsDecision: outcome = needsLook ? .paused : changed ? .broughtIn : .upToDate
        case .signedOut: outcome = .signedOut
        case .offline: outcome = .offline
        case .outOfTime: outcome = .outOfTime
        case .busy: outcome = .busy
        case .failed: outcome = .failed
        }
        return BackgroundRun(at: at, outcome: outcome, changed: changed, needsLook: needsLook)
    }

    // MARK: The record

    private static func record(_ run: BackgroundRun) {
        let defaults = UserDefaults.standard
        defaults.set(run.at.timeIntervalSince1970, forKey: atKey)
        defaults.set(run.outcome.rawValue, forKey: resultKey)
        // What the next open must still do is kept until it has done it.
        if run.changed { defaults.set(true, forKey: changedKey) }
        if run.needsLook { defaults.set(true, forKey: needsLookKey) }
    }

    /// The last run, if there has been one. A record this build can't
    /// read (an older one's sentence) is none.
    static func lastRun() -> BackgroundRun? {
        let defaults = UserDefaults.standard
        let at = defaults.double(forKey: atKey)
        guard at > 0, let outcome = defaults.string(forKey: resultKey).flatMap(BackgroundRun.Outcome.init(rawValue:))
        else { return nil }
        return BackgroundRun(
            at: Date(timeIntervalSince1970: at), outcome: outcome, changed: defaults.bool(forKey: changedKey),
            needsLook: defaults.bool(forKey: needsLookKey))
    }

    /// What runs since the last open left for the screens: changes to show,
    /// and decisions to bring up. Cleared once taken.
    static func takePending() -> (changed: Bool, needsLook: Bool) {
        let defaults = UserDefaults.standard
        let pending = (changed: defaults.bool(forKey: changedKey), needsLook: defaults.bool(forKey: needsLookKey))
        defaults.removeObject(forKey: changedKey)
        defaults.removeObject(forKey: needsLookKey)
        return pending
    }
}
