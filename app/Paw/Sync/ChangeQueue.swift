import Foundation
import Observation

/// The changes saved on this iPhone and not sent yet. Each change to the
/// queue is announced on the live store like any other write.
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

    /// Takes back the newest change. `false`, with `problem` said, if it
    /// failed.
    @discardableResult
    func undoLast() -> Bool {
        change { sync in try ChangeQueue.touched(sync.undoLast()) }
    }

    @discardableResult
    func discard(_ id: String) -> Bool {
        change { sync in try ChangeQueue.touched(sync.discard(id: id)) }
    }

    @discardableResult
    func discardAll() -> Bool {
        let entries = unpushed.map(\.entry)
        return change { sync in
            try sync.discardAll()
            return entries
        }
    }

    /// The entries the changes `items` took back touched.
    private static func touched(_ items: [QueueItem]) -> [String] {
        items.flatMap { [$0.previous, $0.entry].compactMap { $0 } }
    }

    /// Every change to the queue: it takes the store lock, so it runs
    /// inside background time (`BackgroundTime`). `run` gives the entries
    /// it touched.
    private func change(_ run: (StoreSync) throws -> [String]) -> Bool {
        let entries: [String]
        do {
            entries = try BackgroundTime.run("Change") { try run(sync) }
        } catch {
            problem = describe(error)
            return false
        }
        live.announce(.wrote(entries: entries, renamed: nil))
        return true
    }
}
