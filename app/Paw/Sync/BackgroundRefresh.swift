import BackgroundTasks
import Foundation
import UIKit

/// The last background check: when, and how it went (never an entry name
/// or an error's text). Kept in the app's own defaults, which can be read
/// and written while the iPhone is locked; the store can't (it is
/// `complete`-protected).
struct BackgroundRun: Equatable {
    enum Outcome: String {
        case locked, noStore, notSignedIn, notLinked, upToDate, broughtIn, paused, signedOut, offline, outOfTime,
             busy, failed

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
/// iOS decides when it runs, if at all.
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

    /// Replaces any request already made.
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
        if LaunchArgument.simBackgroundRefresh.bool {
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
            let opened = run.changed && !stop.isStopped() ? openStore() : nil
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    NotificationCenter.default.removeObserver(locking)
                    record(run)
                    Task { @MainActor in
                        // Stopped (time is up, or the phone locked): end the
                        // task now. The next open publishes the suggestions.
                        if let opened, !stop.isStopped() {
                            await AutoFillSuggestions(stateDir: opened.stateDir).publish(opened.store).value
                        }
                        NotificationCenter.default.post(name: finished, object: nil)
                        done(run)
                    }
                }
            }
        }
    }

    /// Blocks for at most `budgetSeconds`.
    private static func runNow(stop: BackgroundStop) -> BackgroundRun {
        guard let location = try? StoreLocation.standard(), location.isSetUp else {
            return skipped(.noStore)
        }
        guard let core = GitHubSignInSource.account(stateDir: location.stateDir.path) else {
            return skipped(.notSignedIn)
        }
        let report = core.backgroundRefreshStoppable(seconds: budgetSeconds, stop: stop)
        let run = summary(of: report, at: Date())
        if run.success, report.integration != nil {
            // The store matches the server as of now, as after a check.
            LastChecked.record(run.at, repoDir: location.repoDir.path)
        }
        return run
    }

    private static func openStore() -> (store: PassStore, stateDir: String)? {
        guard let location = try? StoreLocation.standard(),
              let store = try? PassStore(stateDir: location.stateDir.path) else { return nil }
        return (store, location.stateDir.path)
    }

    private static func skipped(_ outcome: BackgroundRun.Outcome) -> BackgroundRun {
        BackgroundRun(at: Date(), outcome: outcome, changed: false, needsLook: false)
    }

    static func summary(of report: BackgroundRefreshReport, at: Date) -> BackgroundRun {
        let changed = report.integration.map { $0.fastForwarded > 0 || $0.replayed > 0 } ?? false
        // Held-back changes are said on Home already; they stop nothing here.
        let needsLook = report.decisions.contains { decision in
            if case .heldBack = decision { return false }
            return true
        }
        let outcome: BackgroundRun.Outcome
        switch report.outcome {
        case .upToDate: outcome = .upToDate
        case .updated: outcome = .broughtIn
        case .needsDecision: outcome = needsLook ? .paused : changed ? .broughtIn : .upToDate
        case .outOfTime: outcome = .outOfTime
        case .failed(.remote(.notSignedIn)): outcome = .notSignedIn
        case .failed(.remote(.notLinked)): outcome = .notLinked
        case .failed(.remote(.signedOut)): outcome = .signedOut
        case .failed(.remote(.unreachable)): outcome = .offline
        case .failed(.storeBusy): outcome = .busy
        case .failed: outcome = .failed
        }
        return BackgroundRun(at: at, outcome: outcome, changed: changed, needsLook: needsLook)
    }

    private static func record(_ run: BackgroundRun) {
        let defaults = UserDefaults.standard
        defaults.set(run.at.timeIntervalSince1970, forKey: atKey)
        defaults.set(run.outcome.rawValue, forKey: resultKey)
        // What the next open must still do is kept until it has done it.
        if run.changed { defaults.set(true, forKey: changedKey) }
        if run.needsLook { defaults.set(true, forKey: needsLookKey) }
    }

    static func lastRun() -> BackgroundRun? {
        let defaults = UserDefaults.standard
        let at = defaults.double(forKey: atKey)
        guard at > 0, let result = defaults.string(forKey: resultKey),
              let outcome = BackgroundRun.Outcome(rawValue: result) else { return nil }
        return BackgroundRun(
            at: Date(timeIntervalSince1970: at), outcome: outcome, changed: defaults.bool(forKey: changedKey),
            needsLook: defaults.bool(forKey: needsLookKey))
    }

    /// The store is removed from this iPhone: the last run and what it
    /// left for the next open go with it.
    static func forgetLastRun() {
        let defaults = UserDefaults.standard
        for key in [atKey, resultKey, changedKey, needsLookKey] {
            defaults.removeObject(forKey: key)
        }
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
