import Foundation
import UIKit

/// An entry that was open on screen and has been closed: its text is
/// wiped; the screen says why and offers to open it again.
struct ClosedEntry: Equatable {
    enum Reason: Equatable {
        /// paw was in the background for 2 minutes or more.
        case away
        case deviceLocked
        /// Its Face ID copy ended (time ran out, or "Lock this password").
        case copyEnded
    }

    let ref: EntryRef
    let reason: Reason
}

/// When open entries close (design 2.5): after 2 minutes away (see
/// `scenePhaseChanged`), when the iPhone locks, and when the entry's Face
/// ID copy ends while it is on screen.
extension AppModel {
    /// The iPhone is locking: close every open entry. Its data protection
    /// is about to make the files unreadable anyway.
    func watchDeviceLock() {
        deviceLockWatch = NotificationCenter.default.addObserver(
            forName: UIApplication.protectedDataWillBecomeUnavailableNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.closeAllSessions(because: .deviceLocked)
            }
        }
    }

    /// An entry that opened with a Face ID copy closes once the copy has
    /// ended (on the copies' clock), as its row moves to "needs your key".
    func closeEntriesWhoseCopyEnded() {
        for (id, session) in sessions where session.hadCopy && copies.copy(of: session.ref.path) == nil {
            closeSession(id, because: .copyEnded)
        }
    }

    func closedEntry(_ id: UUID) -> ClosedEntry? {
        closedEntries[id]
    }

    /// "Lock this password" on an open entry: ends its Face ID copy only,
    /// closes it and goes back to the list. Lock now in Settings ends all.
    /// A copy that could not be ended is said on the list, not called
    /// locked.
    func lockEntry(_ session: OpenSession) {
        let name = session.ref.name
        do {
            try copies.tryChange { try $0.forgetEntry(entry: session.ref.path) }
            toast = Toast(text: "Locked. \(name) needs your key again.")
        } catch {
            notice = Notice(problem: "Couldn't lock \(name)", nextStep: "Use Lock now in Settings.")
        }
        leave(.entry(session.id))
    }

    /// "Open again" on a closed entry: back to the list, then open it as a
    /// tap on its row would (Face ID if it still has a copy, else the key).
    func reopen(_ id: UUID) {
        guard let closed = closedEntries[id] else { return }
        if path.last == .entry(id) {
            setPath(Array(path.dropLast()))
        }
        closedEntries[id] = nil
        open(closed.ref)
    }
}
