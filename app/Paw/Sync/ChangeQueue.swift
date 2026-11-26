import Foundation
import Observation

/// The changes saved on this iPhone and not sent yet, as the core's queue
/// holds them, read again whenever the store's entries may have changed.
/// Undo and discard change it, a send empties it; each change is announced
/// on the live store like any other write.
@MainActor @Observable
final class ChangeQueue {
    /// Newest first.
    private(set) var unpushed: [QueueItem] = []
    /// Why the queue could not be read or changed, said on the list.
    private(set) var problem: String?
    /// The last read failed: `unpushed` is what was read before.
    private(set) var unreadable = false

    @ObservationIgnored private let sync: StoreSync
    @ObservationIgnored private let live: LiveStore

    init(sync: StoreSync, live: LiveStore) {
        self.sync = sync
        self.live = live
        read()
        live.changed.subscribe(self) { queue, changed in
            if changed.change.rewritesEntries {
                queue.read()
            }
        }
    }

    /// How many changes would be lost by removing the store; `nil` when
    /// the queue can't be read, so nobody is told nothing will be lost.
    var unsentCount: Int? {
        unreadable ? nil : unpushed.count
    }

    private func read() {
        do {
            unpushed = try sync.unpushed()
            unreadable = false
            problem = nil
        } catch {
            unreadable = true
            problem = describe(error)
        }
    }

    // MARK: Changing it

    /// Takes back the newest change. `task` names the time asked of iOS to
    /// finish it. `false`, with `problem` said, if it failed.
    @discardableResult
    func undoLast(task: String) -> Bool {
        change { try BackgroundTime.run(task) { try sync.undoLast() } }
    }

    @discardableResult
    func discard(_ id: String) -> Bool {
        change { try BackgroundTime.run("Change") { try sync.discard(id: id) } }
    }

    @discardableResult
    func discardAll() -> Bool {
        let entries = unpushed.map(\.entry)
        do {
            try BackgroundTime.run("Change") { try sync.discardAll() }
        } catch {
            problem = describe(error)
            return false
        }
        live.announce(.wrote(entries: entries, renamed: nil))
        return true
    }

    private func change(_ run: () throws -> [QueueItem]) -> Bool {
        let changed: [QueueItem]
        do {
            changed = try run()
        } catch {
            problem = describe(error)
            return false
        }
        live.announce(.wrote(entries: changed.flatMap { [$0.previous, $0.entry].compactMap { $0 } }, renamed: nil))
        return true
    }
}
