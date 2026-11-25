import BackgroundTasks
import Foundation
import UIKit

/// The last background check, as Sync says it: when, and how it went in a
/// few plain words (never an entry name or an error's text). Kept in the
/// app's own defaults, which can be read and written while the iPhone is
/// locked; the store can't (it is `complete`-protected).
struct BackgroundRun: Equatable {
    let at: Date
    let result: String
    /// Changes came in that the list has not shown yet.
    let changed: Bool
    /// Something only the person can decide was found and left alone.
    let needsLook: Bool
    /// Ended as it should (for `setTaskCompleted`).
    let success: Bool
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
    private static let successKey = "pawBackgroundRunSuccess"

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
        let handoff = TaskHandoff(task: task)
        start(stop: stop) { run in
            handoff.task.setTaskCompleted(success: run.success)
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
            let run = readable ? runNow(stop: stop) : skipped("iPhone was locked")
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
            return skipped("No store yet")
        }
        #if targetEnvironment(simulator)
        guard let core = SimGitHubLaunch.shared.makeCore(stateDir: location.stateDir.path) else {
            return skipped("Not signed in to GitHub")
        }
        #else
        let core = GitHub.open(stateDir: location.stateDir.path, vault: KeychainTokenVault())
        #endif
        let report = core.backgroundRefreshStoppable(seconds: budgetSeconds, stop: stop)
        let run = summary(of: report, at: Date())
        if run.success, report.integration != nil {
            // The store matches the server as of now, as after a check.
            UserDefaults.standard.set(run.at.timeIntervalSince1970, forKey: SyncModel.checkedKey(location.repoDir.path))
        }
        return run
    }

    private static func openStore() -> PassStore? {
        guard let location = try? StoreLocation.standard() else { return nil }
        return try? PassStore(dir: location.repoDir.path)
    }

    private static func skipped(_ why: String) -> BackgroundRun {
        BackgroundRun(at: Date(), result: why, changed: false, needsLook: false, success: true)
    }

    /// The core's report in Sync's words.
    static func summary(of report: BackgroundRefreshReport, at: Date) -> BackgroundRun {
        let changed = report.integration.map { $0.fastForwarded > 0 || $0.replayed > 0 } ?? false
        // Held-back changes are said on Home already; they stop nothing here.
        let needsLook = report.decisions.contains { decision in
            if case .heldBack = decision { return false }
            return true
        }
        let run = { (result: String, success: Bool) in
            BackgroundRun(at: at, result: result, changed: changed, needsLook: needsLook, success: success)
        }
        switch report.outcome {
        case .notSignedIn: return run("Not signed in to GitHub", true)
        case .notLinked: return run("No store chosen", true)
        case .upToDate: return run("Up to date", true)
        case .updated: return run("Brought in changes", true)
        case .needsDecision:
            if needsLook { return run("Paused", true) }
            return run(changed ? "Brought in changes" : "Up to date", true)
        case .signedOut: return run("GitHub sign-in ended", true)
        case .offline: return run("Couldn't reach GitHub", false)
        case .outOfTime: return run("Ran out of time", false)
        case .busy: return run("Store was busy", false)
        case .failed: return run("Failed", false)
        }
    }

    // MARK: The record

    private static func record(_ run: BackgroundRun) {
        let defaults = UserDefaults.standard
        defaults.set(run.at.timeIntervalSince1970, forKey: atKey)
        defaults.set(run.result, forKey: resultKey)
        defaults.set(run.success, forKey: successKey)
        // What the next open must still do is kept until it has done it.
        if run.changed { defaults.set(true, forKey: changedKey) }
        if run.needsLook { defaults.set(true, forKey: needsLookKey) }
    }

    /// The last run, if there has been one.
    static func lastRun() -> BackgroundRun? {
        let defaults = UserDefaults.standard
        let at = defaults.double(forKey: atKey)
        guard at > 0, let result = defaults.string(forKey: resultKey) else { return nil }
        return BackgroundRun(
            at: Date(timeIntervalSince1970: at), result: result, changed: defaults.bool(forKey: changedKey),
            needsLook: defaults.bool(forKey: needsLookKey), success: defaults.bool(forKey: successKey))
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

/// Carries the task to the main queue once; iOS's task object is not
/// marked `Sendable`.
private struct TaskHandoff: @unchecked Sendable {
    let task: BGAppRefreshTask
}
