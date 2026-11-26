import Foundation
import Observation
import UIKit

/// A broadcast on the main actor. Each listener belongs to an owner and
/// hears every value sent, in the order the listeners came, for as long as
/// its owner lives: no listener keeps its owner alive.
@MainActor
final class Signal<Value> {
    private struct Listener {
        weak var owner: AnyObject?
        let hear: @MainActor (AnyObject, Value) -> Void
    }

    private var listeners: [Listener] = []

    nonisolated init() {}

    func subscribe<Owner: AnyObject>(_ owner: Owner, _ hear: @escaping @MainActor (Owner, Value) -> Void) {
        listeners.append(Listener(owner: owner) { owner, value in
            guard let owner = owner as? Owner else { return }
            hear(owner, value)
        })
    }

    func send(_ value: Value) {
        listeners.removeAll { $0.owner == nil }
        for listener in listeners {
            if let owner = listener.owner {
                listener.hear(owner, value)
            }
        }
    }
}

/// Why the store on disk changed, and what changed.
enum StoreChange {
    /// A save, delete, undo or discard on this iPhone: the entries it
    /// touched, and the rename if it was one.
    case wrote(entries: [String], renamed: Renamed?)
    /// A folder's new list of keys was accepted on this iPhone.
    case recipientsAccepted(folder: String)
    /// A sync brought changes in: the core's report, or `nil` for a
    /// background check while paw was away, which keeps none.
    case synced(IntegrateReport?, replaced: Bool)
    /// A re-encrypt of held-back changes ended, however it ended.
    case reencrypted
    /// The store is being removed from this iPhone.
    case removed

    struct Renamed: Equatable {
        let from: String
        let to: String
    }

    /// The entries' files may differ: lists and search read the store again.
    var rewritesEntries: Bool {
        switch self {
        case .wrote, .synced, .reencrypted: true
        case .recipientsAccepted, .removed: false
        }
    }
}

/// The store changed: why, and the store as it is now.
struct StoreChanged {
    let change: StoreChange
    /// Read again after the change; the one before if that failed.
    let store: PassStore
    /// Why the store could not be read again.
    let readProblem: Error?
}

/// An entry was opened on screen, with the key or with Face ID.
struct EntryOpened {
    let entry: OpenedEntry
    let path: String
}

/// What a feature asks of the app's screens.
enum ScreenRequest {
    case show(Route)
    case leave(Route)
    case toast(String)
    case settings
}

/// The store as paw reads it now, and the one signal that it changed.
/// Whatever writes or syncs the store announces it here; lists, the
/// queue, sync, the open form and AutoFill listen.
@MainActor @Observable
final class LiveStore {
    private(set) var store: PassStore
    let repoDir: String
    let changed = Signal<StoreChanged>()
    let opened = Signal<EntryOpened>()

    init(store: PassStore, repoDir: String) {
        self.store = store
        self.repoDir = repoDir
    }

    /// Reads the store again if its entries may differ, then tells every
    /// listener.
    func announce(_ change: StoreChange) {
        var problem: Error?
        if change.rewritesEntries {
            do {
                store = try PassStore(dir: repoDir)
            } catch {
                problem = error
            }
        }
        changed.send(StoreChanged(change: change, store: store, readProblem: problem))
    }
}

/// What only the store paw set up has: AutoFill's suggestions, its state
/// folder and Remove. A fixture has none of it.
struct SetUpStore {
    let location: StoreLocation
    /// Removes the store from this iPhone; why not, if it could not.
    let remove: @MainActor () -> String?
}

/// Everything that belongs to one open store: the store, its Face ID
/// copies, the key, the write side, sync, the GitHub sign-in and AutoFill's
/// upkeep. Complete when built; `AppModel` shows it.
@MainActor
final class StoreSession {
    /// What a session is built from. The set-up store and the simulator's
    /// fixture differ only in these.
    struct Parts {
        let repoDir: String
        /// `nil` where Face ID copies cannot be kept.
        let copies: CopyCache?
        /// The copies' clock, the key port's and the clipboard's.
        let clock: Clock
        let keyPort: KeyPort
        let connect: KeyReader.Connect
        let simulator: SimulatorControls?
        /// The write side: `nil` where the store can't be written.
        let writing: Writing?
        let gitHub: GitHubAccess?
        let setUp: SetUpStore?
    }

    struct Writing {
        let writer: StoreWriter
        /// `nil` if the queue could not be opened; then nothing syncs.
        let sync: StoreSync?
        let stateDir: String
    }

    let live: LiveStore
    let copies: CopyCache?
    let clock: Clock
    let keyPort: KeyPort
    /// The key going in and out, and reading it.
    let reader: KeyReader
    /// Debug-only controls for the simulated hardware, `nil` on a device.
    let simulator: SimulatorControls?
    /// Adding, editing and deleting, and the unpushed queue.
    let edits: StoreEdits?
    /// Sending and checking for changes; `nil` without the queue.
    let syncing: SyncModel?
    let gitHub: GitHubAccess?
    let autoFill: AutoFillUpkeep?
    let setUp: SetUpStore?
    /// Navigation and toasts that sync asks for.
    let requests: Signal<ScreenRequest>
    private var backgroundWatch: NSObjectProtocol?

    /// Opens the store in `parts.repoDir` and builds everything on it.
    init(_ parts: Parts) throws {
        let store = try PassStore(dir: parts.repoDir)
        let live = LiveStore(store: store, repoDir: parts.repoDir)
        self.live = live
        copies = parts.copies
        clock = parts.clock
        keyPort = parts.keyPort
        simulator = parts.simulator
        gitHub = parts.gitHub
        setUp = parts.setUp
        let reader = KeyReader(port: parts.keyPort, connect: parts.connect)
        self.reader = reader
        let requests = Signal<ScreenRequest>()
        self.requests = requests
        let edits = parts.writing.map { StoreEdits(writer: $0.writer, sync: $0.sync, stateDir: $0.stateDir, live: live) }
        self.edits = edits
        if let edits, let sync = parts.writing?.sync {
            syncing = SyncModel(
                sync: sync, queue: edits, live: live, requests: requests, reader: reader, copies: parts.copies,
                gitHub: parts.gitHub)
        } else {
            syncing = nil
        }
        autoFill = parts.setUp.map { setUp in
            AutoFillUpkeep(live: live, codes: OneTimeCodeEntries.open(stateDir: setUp.location.stateDir.path))
        }
        Clipboard.useClock(parts.clock)
        #if DEBUG && targetEnvironment(simulator)
        if let simulator = parts.simulator {
            reader.plugsInSimulatedKey = simulator.plugIn
        }
        #endif
        reader.watch(startsUnplugged: parts.simulator?.startsUnplugged == true)
        #if DEBUG && targetEnvironment(simulator)
        // A session already open (setup's) is kept, with the PIN it holds.
        if reader.present {
            reader.read()
        }
        #endif
        // A background check that ends while paw is on screen; one that
        // ends in the background waits for the next `cameForward`.
        backgroundWatch = NotificationCenter.default.addObserver(
            forName: BackgroundRefresh.finished, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard UIApplication.shared.applicationState != .background else { return }
                self?.syncing?.cameForward()
            }
        }
    }

    /// paw is on screen again: AutoFill's suggestions are written again,
    /// and sync takes what background checks left.
    func cameForward() {
        autoFill?.republish()
        syncing?.cameForward()
    }

    /// The editor or the sync sheet is up.
    var sheetIsUp: Bool {
        edits?.form != nil || syncing?.sheet != nil
    }
}
