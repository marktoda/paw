import CryptoTokenKit
import Foundation

/// Talks to a YubiKey plugged into the USB-C port, through the system's
/// smart card support. One instance holds one exclusive session with the key.
final class UsbCardConnection: CardConnection, @unchecked Sendable {
    enum Failure: LocalizedError {
        case unavailable
        case noKey
        case sessionRefused(String)
        case timedOut(String)

        var errorDescription: String? {
            switch self {
            case .unavailable: return "This device has no smart card support."
            case .noKey: return "No key is plugged in."
            case .sessionRefused(let why): return "The key did not open a session: \(why)"
            case .timedOut(let what): return "Timed out waiting for \(what)."
            }
        }
    }

    private let card: TKSmartCard
    private let onTouchNeeded: @Sendable () -> Void

    /// The names of the smart card readers the system sees right now.
    static func slotNames() -> [String] {
        TKSmartCardSlotManager.default?.slotNames ?? []
    }

    /// Opens a session with the first key found. Blocks; call off the main thread.
    init(onTouchNeeded: @escaping @Sendable () -> Void) throws {
        guard let manager = TKSmartCardSlotManager.default else { throw Failure.unavailable }
        guard let name = manager.slotNames.first else { throw Failure.noKey }

        var slot: TKSmartCardSlot?
        let gotSlot = DispatchSemaphore(value: 0)
        manager.getSlot(withName: name) { found in
            slot = found
            gotSlot.signal()
        }
        guard gotSlot.wait(timeout: .now() + 5) == .success else {
            throw Failure.timedOut("the card reader")
        }
        guard let card = slot?.makeSmartCard() else { throw Failure.noKey }

        var opened = false
        var openError: Error?
        let began = DispatchSemaphore(value: 0)
        card.beginSession { success, error in
            opened = success
            openError = error
            began.signal()
        }
        // Another process holding the key makes this queue; give up instead.
        guard began.wait(timeout: .now() + 10) == .success else {
            throw Failure.timedOut("a session with the key")
        }
        guard opened else {
            throw Failure.sessionRefused(openError?.localizedDescription ?? "unknown")
        }

        self.card = card
        self.onTouchNeeded = onTouchNeeded
    }

    deinit {
        card.endSession()
    }

    func transmit(apdu: Data) throws -> Data {
        var response: Data?
        var failure: Error?
        let done = DispatchSemaphore(value: 0)
        card.transmit(apdu) { data, error in
            response = data
            failure = error
            done.signal()
        }
        // A decrypt waits for the touch, so allow well over the key's own timeout.
        guard done.wait(timeout: .now() + 40) == .success else {
            throw ConnectionError.Failed(message: "Timed out waiting for the key.")
        }
        guard let response else {
            throw ConnectionError.Failed(
                message: failure?.localizedDescription ?? "The key sent no response.")
        }
        return response
    }

    func touchNeeded() {
        onTouchNeeded()
    }
}
