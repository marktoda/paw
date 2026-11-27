import Foundation
import Observation

/// The Face ID copies as paw shows them. Every change to them goes through
/// `change` or `tryChange`, which read the list from the core again
/// whatever the change did, so Home's list, its problem and the entries
/// open from a copy follow it.
@MainActor @Observable
final class LiveCopies {
    /// `nil` where Face ID copies cannot be kept: the list stays empty.
    let cache: CopyCache?
    private(set) var list: [LiveCopy] = []
    /// A problem with the copies themselves, said on Home.
    private(set) var problem: String?
    /// After each read of the list.
    let listed = Signal<Void>()

    init(_ cache: CopyCache?) {
        self.cache = cache
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

    /// Reads the copies again with no change of paw's: time has moved on,
    /// or Face ID found a copy ended.
    func reread() {
        reread(after: nil)
    }

    private func reread(after failure: Error?) {
        if let cache {
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
