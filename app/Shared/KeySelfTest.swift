import Foundation

/// The spike's whole job: prove this process can reach the key and decrypt
/// with it. Used by both the app and the AutoFill extension.
enum KeySelfTest {
    static let message = "paw can decrypt with this key"

    /// Reads the key's identity. Needs no PIN and no touch.
    static func readKey() throws -> CardInfo {
        try readCard(connection: KeyConnection.open(onTouchNeeded: {}))
    }

    /// Encrypts a fixed message to the key, then decrypts it with PIN and touch.
    static func roundTrip(pin: String, onTouchNeeded: @escaping @Sendable () -> Void) throws -> String {
        let connection = try KeyConnection.open(onTouchNeeded: onTouchNeeded)
        let ciphertext = try encryptToCard(connection: connection, plaintext: Data(message.utf8))
        let plaintext = try decrypt(connection: connection, pin: pin, ciphertext: ciphertext)
        return String(decoding: plaintext, as: UTF8.self)
    }
}
