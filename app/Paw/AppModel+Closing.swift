import Foundation
import UIKit
#if !targetEnvironment(simulator)
import CryptoTokenKit
#endif

/// An entry that was open on screen and has been closed: its text is
/// wiped; the screen says why and offers to open it again.
struct ClosedEntry: Equatable {
    enum Reason: Equatable {
        /// paw was in the background for 2 minutes or more.
        case away
        /// The iPhone locked.
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
                self?.deviceLocked()
            }
        }
    }

    func deviceLocked() {
        closeAllSessions(because: .deviceLocked)
    }

    /// An entry that opened with a Face ID copy closes once the copy has
    /// ended (on the copies' clock), as its row moves to "needs your key".
    func closeEntriesWhoseCopyEnded() {
        for (id, session) in sessions where session.hadCopy && liveCopy(session.ref.path) == nil {
            closeSession(id, because: .copyEnded)
        }
    }

    func closedEntry(_ id: UUID) -> ClosedEntry? {
        closedEntries[id]
    }

    /// "Lock this password" on an open entry: ends its Face ID copy only,
    /// closes it and goes back to the list. Lock now in Settings ends all.
    func lockEntry(_ session: OpenSession) {
        do {
            try copies?.forgetEntry(entry: session.ref.path)
        } catch {
            notice = Notice(
                problem: "Couldn't lock \(session.ref.name)",
                nextStep: "Use Lock now in Settings.")
        }
        closeSession(session.id, because: .copyEnded)
        if path.last == .entry(session.id) {
            path.removeLast()
        }
        refreshCopies()
        toast = "Locked. \(session.ref.name) needs your key again."
    }

    /// "Open again" on a closed entry: back to the list, then open it as a
    /// tap on its row would (Face ID if it still has a copy, else the key).
    func reopen(_ id: UUID) {
        guard let closed = closedEntries[id] else { return }
        if path.last == .entry(id) {
            path.removeLast()
        }
        closedEntries[id] = nil
        open(closed.ref)
    }

    #if !targetEnvironment(simulator)
    /// Follows the first reader's card state, so a key that finishes
    /// powering up after its reader appeared is read then, without a
    /// replug. KVO on `TKSmartCardSlot.state`; unverified on hardware.
    func watchSlotState() {
        guard let manager = TKSmartCardSlotManager.default, let name = manager.slotNames.first else { return }
        manager.getSlot(withName: name) { [weak self] slot in
            guard let slot else { return }
            let watch = slot.observe(\.state, options: [.new]) { [weak self] slot, _ in
                guard slot.state == .validCard else { return }
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        guard let self, self.keyPort.session() == nil else { return }
                        self.plugIn()
                    }
                }
            }
            let handoff = SlotWatchHandoff(watch: watch)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.slotStateWatch = handoff.watch
                }
            }
        }
    }
    #endif
}

#if !targetEnvironment(simulator)
private struct SlotWatchHandoff: @unchecked Sendable {
    let watch: NSKeyValueObservation
}
#endif
