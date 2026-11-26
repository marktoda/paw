import Foundation
import Security

/// Where the core keeps the GitHub sign-in: one keychain generic-password
/// item per name (service `paw.github`, account the name), readable after
/// the first unlock so a background refresh can use it with the phone
/// locked, never synchronised, never leaving this device, and with no
/// Face ID or passcode on it. It lives in the app's keychain group
/// (`KeychainGroup`), which the AutoFill extension can be given later.
///
/// The secret passes through here as bytes on its way to or from the
/// keychain and is kept nowhere else; nothing here logs it or puts it in
/// an error.
///
/// Not verified yet: an unsigned simulator build has no keychain
/// entitlement (-34018), so the simulator uses the core's `SimTokenVault`
/// and only a signed device build runs this.
final class KeychainTokenVault: TokenVault, @unchecked Sendable {
    static let service = "paw.github"

    private let lock = NSLock()

    init() {}

    func store(name: String, secret: Data) throws {
        let group = try Self.group()
        try lock.withLock {
            let query = Self.query(name: name, group: group)
            // Replaced in one update, so a reader sees the old item or the
            // new one, never neither.
            let update: [String: Any] = [
                kSecValueData as String: secret,
                kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            ]
            let updated = SecItemUpdate(query as CFDictionary, update as CFDictionary)
            switch updated {
            case errSecSuccess:
                return
            case errSecItemNotFound:
                var add = query
                add[kSecValueData as String] = secret
                add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
                let added = SecItemAdd(add as CFDictionary, nil)
                guard added == errSecSuccess else { throw Self.failure(added, "keeping the GitHub sign-in") }
            default:
                throw Self.failure(updated, "replacing the GitHub sign-in")
            }
        }
    }

    func load(name: String) throws -> Data? {
        let group = try Self.group()
        return try lock.withLock {
            var query = Self.query(name: name, group: group)
            query[kSecReturnData as String] = true
            query[kSecMatchLimit as String] = kSecMatchLimitOne
            var result: CFTypeRef?
            let status = SecItemCopyMatching(query as CFDictionary, &result)
            switch status {
            case errSecSuccess:
                guard let data = result as? Data else { throw VaultError.Failed(message: "the keychain item held no data") }
                return data
            case errSecItemNotFound:
                return nil
            default:
                throw Self.failure(status, "reading the GitHub sign-in")
            }
        }
    }

    func remove(name: String) throws {
        let group = try Self.group()
        try lock.withLock {
            let status = SecItemDelete(Self.query(name: name, group: group) as CFDictionary)
            guard status == errSecSuccess || status == errSecItemNotFound else {
                throw Self.failure(status, "removing the GitHub sign-in")
            }
        }
    }

    private static func query(name: String, group: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: name,
            kSecAttrAccessGroup as String: group,
            kSecAttrSynchronizable as String: false,
            kSecUseDataProtectionKeychain as String: true,
        ]
    }

    private static func group() throws -> String {
        do {
            return try KeychainGroup.resolve()
        } catch let failure as EnclaveFailure {
            throw VaultError.Failed(message: failure.detail)
        }
    }

    private static func failure(_ status: OSStatus, _ doing: String) -> VaultError {
        VaultError.Failed(message: "the keychain refused \(doing) (OSStatus \(status))")
    }
}
