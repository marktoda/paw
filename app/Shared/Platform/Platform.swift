import CryptoTokenKit
import Foundation

/// The hardware one paw process runs on: where the key comes from, the
/// clock, the Face ID keystore, and the process's name in the key log.
/// There are two: the device's, and in the simulator's debug builds a
/// simulated one. Each process entry point (the app's launch, the
/// extension's request, the fixture and the AutoFill harness) picks one
/// and hands it down; nothing below an entry point picks either.
struct Platform {
    let process: StoreProcess
    let keys: any KeyConnector
    /// The copies' clock, the key port's and the clipboard's.
    let clock: Clock
    private let makeCopies: (_ dir: String) -> CopyCache?
    private let wipeKeychain: () throws -> Void

    private init(
        process: StoreProcess, keys: any KeyConnector, clock: Clock, copies: @escaping (_ dir: String) -> CopyCache?,
        resetKeychain: @escaping () throws -> Void
    ) {
        self.process = process
        self.keys = keys
        self.clock = clock
        makeCopies = copies
        wipeKeychain = resetKeychain
    }

    /// The phone: the key on the USB-C port, the system's clocks, and the
    /// Face ID copies sealed by the Secure Enclave.
    static func device(_ process: StoreProcess) -> Platform {
        let clock = SystemClock()
        return Platform(
            process: process, keys: UsbKeySource(), clock: clock,
            copies: { dir in
                // Without paw's keychain group there are no copies, and
                // every open uses the key (Settings says so). Face ID
                // problems surface as the core's errors when a copy is
                // sealed or opened.
                guard (try? KeychainGroup.shared()) != nil else { return nil }
                return CopyCache(dir: dir, keystore: EnclaveKeystore(), clock: clock)
            },
            resetKeychain: { try EnclaveKeystore().deleteCopyKeys() })
    }

    #if DEBUG && targetEnvironment(simulator)
    static func simulated(
        _ process: StoreProcess, keys: SimulatedKeySource, clock: Clock, keystore: DeviceKeystore?
    ) -> Platform {
        Platform(
            process: process, keys: keys, clock: clock,
            copies: { dir in keystore.map { CopyCache(dir: dir, keystore: $0, clock: clock) } },
            resetKeychain: { try keystore?.reset() })
    }
    #endif

    /// The Face ID copies kept in `dir`; `nil` where this platform keeps
    /// none, and then every open uses the key.
    func copies(in dir: URL) -> CopyCache? {
        makeCopies(dir.path)
    }

    /// A key port on this platform's clock. With `keyLog`, it writes this
    /// process's half of the key log in that state directory from before
    /// the key is first read.
    func keyPort(keyLog stateDir: String?) -> KeyPort {
        let port = KeyPort(clock: clock)
        if let stateDir {
            port.setKeyLog(stateDir: stateDir, process: process.rawValue)
        }
        return port
    }

    /// Deletes the Face ID copies' keychain items (the seal key, the index
    /// key and the enrolment marker), whatever Lock now managed and even
    /// with no Face ID copies on this iPhone.
    func resetKeychain() throws {
        try wipeKeychain()
    }
}

/// Where the key comes from: the YubiKey on the USB-C port, or a simulated
/// key.
protocol KeyConnector: Sendable {
    /// The readers there are now, by name, for Key diagnostics.
    var readerNames: [String] { get }

    /// A connection to the key that is in, for its session
    /// (`KeyPort.pluggedIn`). Blocks: call it off the main thread.
    func connect() throws -> CardConnection

    /// Follows whether a key is in: `changed` hears it now and on every
    /// change, and again when a key that is in becomes readable. It
    /// follows for as long as what it returns is kept; `nil` where this
    /// device can't use USB keys at all.
    @MainActor func follow(_ changed: @escaping @MainActor (_ present: Bool) -> Void) -> AnyObject?

    /// `job` as a key task on this key runs it: as it is, but for the
    /// simulated key, which can finish an open just after its Cancel.
    @MainActor func adapted<Step>(_ job: KeyJob<Step>) -> KeyJob<Step>
}

extension KeyConnector {
    @MainActor func adapted<Step>(_ job: KeyJob<Step>) -> KeyJob<Step> {
        job
    }
}

/// The YubiKey on the USB-C port, through the system's smart card support.
struct UsbKeySource: KeyConnector {
    var readerNames: [String] {
        UsbCardConnection.slotNames()
    }

    func connect() throws -> CardConnection {
        try UsbCardConnection()
    }

    @MainActor func follow(_ changed: @escaping @MainActor (Bool) -> Void) -> AnyObject? {
        guard let manager = TKSmartCardSlotManager.default else { return nil }
        return ReaderWatch(manager: manager, changed: changed)
    }
}

/// Minimal, and unverified on hardware: a reader appearing plugs in, the
/// list emptying unplugs, and the first reader's card becoming valid says
/// so again, so a key that finishes powering up after its reader appeared
/// is read then, without a replug. A key pulled out mid-command is also
/// caught by the core as a reader failure.
@MainActor
private final class ReaderWatch {
    private let changed: @MainActor (Bool) -> Void
    private var slotWatch: NSKeyValueObservation?
    private var slotStateWatch: NSKeyValueObservation?

    init(manager: TKSmartCardSlotManager, changed: @escaping @MainActor (Bool) -> Void) {
        self.changed = changed
        // Called on whatever thread the system changes the list on.
        slotWatch = manager.observe(\.slotNames, options: [.initial, .new]) { [weak self] manager, _ in
            let present = !manager.slotNames.isEmpty
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.readersChanged(present: present)
                }
            }
        }
    }

    private func readersChanged(present: Bool) {
        if present {
            watchSlotState()
        } else {
            slotStateWatch = nil
        }
        changed(present)
    }

    private func watchSlotState() {
        guard let manager = TKSmartCardSlotManager.default, let name = manager.slotNames.first else { return }
        manager.getSlot(withName: name) { [weak self] slot in
            guard let slot else { return }
            let watch = slot.observe(\.state, options: [.new]) { [weak self] slot, _ in
                guard slot.state == .validCard else { return }
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        self?.changed(true)
                    }
                }
            }
            let handoff = Handoff(value: watch)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.slotStateWatch = handoff.value
                }
            }
        }
    }
}
