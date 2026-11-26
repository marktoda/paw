import Foundation

/// Where the key comes from. On a device it is the YubiKey on the USB-C
/// port. In the simulator's debug builds, which cannot see hardware, it is
/// a simulated key so every screen can still be reached and tested.
enum KeyConnection {
    /// A connection for the key session (`KeyPort.pluggedIn`). Blocks; call
    /// off the main thread.
    static func open() throws -> CardConnection {
        #if DEBUG && targetEnvironment(simulator)
        return try SimulatedKeyHolder.shared.open()
        #else
        return try UsbCardConnection()
        #endif
    }

    /// The simulated key is out at launch (`-pawSimUnplugged`). Always
    /// `false` on a device, where the system says whether a key is in.
    static var startsUnplugged: Bool {
        #if DEBUG && targetEnvironment(simulator)
        return SimulatedKeyHolder.shared.startsUnplugged
        #else
        return false
        #endif
    }

    static var readerNames: [String] {
        #if DEBUG && targetEnvironment(simulator)
        return ["Simulated key"]
        #else
        return UsbCardConnection.slotNames()
        #endif
    }
}
