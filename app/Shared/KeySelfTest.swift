import Foundation

#if DEBUG
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

    /// Raw checks of how the key behaves, for building the key session.
    /// Needs the PIN and three touches.
    static func runChecks(pin: String, onTouchNeeded: @escaping @Sendable () -> Void) throws -> String {
        format(cardChecks(connection: try KeyConnection.open(onTouchNeeded: onTouchNeeded), pin: pin))
    }

    /// Asks for one signature nobody should touch for.
    static func runTouchTimeoutCheck(pin: String) throws -> String {
        format(cardTouchTimeoutCheck(connection: try KeyConnection.open(onTouchNeeded: {}), pin: pin))
    }

    private static func format(_ results: [CheckResult]) -> String {
        results
            .map { "\($0.status)  \($0.millis) ms  \($0.responseLen) B  \($0.name)\n    \($0.command)" }
            .joined(separator: "\n")
    }
}
#endif
