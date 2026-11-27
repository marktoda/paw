import Foundation
import Observation

/// The store's Face ID copies as paw shows them, and their setting. The
/// list is read from the core again after every change made through
/// `change` or `tryChange`, every store change and every entry opened, so
/// Home's list, its problem and the entries open from a copy follow it.
@MainActor @Observable
final class LiveCopies {
    /// `nil` where Face ID copies cannot be kept: the list stays empty.
    let cache: CopyCache?
    private(set) var list: [LiveCopy] = []
    /// A problem with the copies themselves, said on Home.
    private(set) var problem: String?
    /// Opening with the key keeps a copy (Settings › Face ID). The core
    /// keeps it beside the copies, so the AutoFill extension honours it too.
    private(set) var enabled = false
    /// After each read of the list.
    let listed = Signal<Void>()

    init(_ cache: CopyCache?, live: LiveStore) {
        self.cache = cache
        live.changed.subscribe(self) { copies, changed in
            copies.storeChanged(changed)
        }
        live.opened.subscribe(self) { copies, _ in
            copies.reread()
        }
        change { try $0.sweep() }
    }

    func copy(of entry: String) -> LiveCopy? {
        list.first { $0.entry == entry }
    }

    /// Runs `body` on the copies, then reads them again. What it throws is
    /// said on Home, unless the list itself can't be read.
    func change(_ body: (CopyCache) throws -> Void) {
        var failure: Error?
        if let cache {
            do {
                try body(cache)
            } catch {
                failure = error
            }
        }
        reread(after: failure)
    }

    /// `change` for a caller that says itself what went wrong: what `body`
    /// throws is thrown on, once the copies are read again.
    func tryChange(_ body: (CopyCache) throws -> Void) throws {
        defer { reread(after: nil) }
        if let cache {
            try body(cache)
        }
    }

    /// Off ends every copy now and keeps none after.
    func setEnabled(_ on: Bool) throws {
        try tryChange { try $0.setCopiesEnabled(enabled: on) }
    }

    /// Reads the copies again with no change of paw's: time has moved on,
    /// or Face ID found a copy ended.
    func reread() {
        reread(after: nil)
    }

    /// Every change reads the list again, since a save can keep a copy and
    /// a sync can end one. A copy of an entry that is gone (a discarded
    /// add, a rename, a delete) is removed, or it would sit on Home until
    /// its time ran out; with no new read of the store nothing is removed.
    private func storeChanged(_ changed: StoreChanged) {
        guard changed.change.rewritesEntries, changed.readProblem == nil else {
            reread()
            return
        }
        let exists = Set(changed.store.allEntries().map(\.path))
        let gone = list.map(\.entry).filter { !exists.contains($0) }
        change { cache in
            var failure: Error?
            for entry in gone {
                do {
                    try cache.forgetEntry(entry: entry)
                } catch {
                    failure = error
                }
            }
            if let failure { throw failure }
        }
    }

    private func reread(after failure: Error?) {
        if let cache {
            enabled = cache.copiesEnabled()
            do {
                list = try cache.list()
                problem = failure.map { describe($0) }
            } catch {
                list = []
                problem = describe(error)
            }
        } else {
            list = []
        }
        listed.send(())
    }
}
