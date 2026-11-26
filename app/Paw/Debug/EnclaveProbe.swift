#if DEBUG
import Foundation
import Security

enum EnclaveProbe {
    /// Round-trips 32 random bytes through a throwaway Secure Enclave key
    /// with no Face ID constraint: shows whether this device (or simulator)
    /// has a Secure Enclave and ECIES works, independent of Face ID.
    static func roundTrip() -> String {
        var error: Unmanaged<CFError>?
        guard let access = SecAccessControlCreateWithFlags(
            kCFAllocatorDefault, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, [.privateKeyUsage], &error
        ) else {
            return "fail: \(EnclaveFailure.from(error, while: "making the access control").detail)"
        }
        let attributes: [String: Any] = [
            kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
            kSecAttrKeySizeInBits as String: 256,
            kSecAttrTokenID as String: kSecAttrTokenIDSecureEnclave,
            kSecPrivateKeyAttrs as String: [
                kSecAttrIsPermanent as String: false,
                kSecAttrAccessControl as String: access,
            ] as [String: Any],
        ]
        guard let key = SecKeyCreateRandomKey(attributes as CFDictionary, &error) else {
            return "fail: \(EnclaveFailure.from(error, while: "making a Secure Enclave key").detail)"
        }
        guard let publicKey = SecKeyCopyPublicKey(key) else { return "fail: no public key" }
        var bytes = [UInt8](repeating: 0, count: 32)
        defer { wipe(&bytes) }
        guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else {
            return "fail: no random bytes"
        }
        do {
            let sealed = try EnclaveKeystore.encrypt(Data(bytes), to: publicKey)
            guard let opened = SecKeyCreateDecryptedData(key, EnclaveKeystore.algorithm, sealed as CFData, &error) as Data? else {
                return "fail: \(EnclaveFailure.from(error, while: "decrypting").detail)"
            }
            return opened == Data(bytes) ? "pass (\(sealed.count)-byte ciphertext)" : "fail: decrypted to other bytes"
        } catch let failure as EnclaveFailure {
            return "fail: \(failure.detail)"
        } catch {
            return "fail"
        }
    }
}
#endif
