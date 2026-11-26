import Foundation
import LocalAuthentication
import Security

/// The Secure Enclave behind Face ID copies: the device half of the core's
/// `DeviceKeystore` seam (core/src/copies/seams.rs).
///
/// - The seal key is a P-256 key made inside the Secure Enclave. Its
///   private half can only be used after a Face ID match against the faces
///   enrolled when it was made (`.biometryCurrentSet`), never with the
///   passcode, and only while the phone has a passcode
///   (`WhenPasscodeSetThisDeviceOnly`).
/// - `seal` encrypts to the public half (ECIES) and never prompts.
/// - `unseal` asks for Face ID with a fresh `LAContext`, then decrypts with
///   the private half using that context, so the prompt is shown exactly
///   once per call. It blocks until Face ID answers: never call it on the
///   main thread (it refuses to).
/// - `indexKey` is a random 32-byte keychain secret readable without a
///   prompt while the phone is unlocked.
///
/// What `seal` returns: `[1][label length][label][ECIES ciphertext]`, where
/// the label is the key's `kSecAttrApplicationLabel` (SHA-1 of its public
/// key, not secret). `unseal` finds exactly the key that sealed, so a key
/// made since (by a reset or by the other process) is never tried, and a
/// missing key is unambiguous: `KeyGone`.
///
/// Stateless apart from a lock that serialises key creation in this
/// process; safe to call from any thread.
final class EnclaveKeystore: DeviceKeystore, @unchecked Sendable {
    private let namespace: String

    init() {
        namespace = "com.marktoda.paw.copies"
    }

    #if DEBUG
    /// Keys of the check screen's own (`app/Paw/Debug`), so it never
    /// touches the real copies' keys.
    init(namespace: String) {
        self.namespace = namespace
    }
    #endif

    private let lock = NSLock()

    static let algorithm = SecKeyAlgorithm.eciesEncryptionCofactorVariableIVX963SHA256AESGCM
    private static let formatVersion: UInt8 = 1

    private var sealTag: Data { Data("\(namespace).seal".utf8) }
    private var service: String { namespace }
    private static let indexAccount = "index-key"
    /// The Face ID enrolment state this process saw for the seal key.
    ///
    /// One record per process (app, AutoFill extension), because iOS gives
    /// each bundle id a different enrolment hash for the same faces: a
    /// record the app wrote would always look "changed" to the extension.
    private static let enrolmentAccount: String = {
        "seal-key-enrolment." + (Bundle.main.bundleIdentifier ?? "unknown")
    }()
    /// The single record shared by both processes in earlier builds.
    private static let legacyEnrolmentAccount = "seal-key-enrolment"

    func seal(secret: Data) throws -> Data {
        try core { try sealDetailed(secret) }
    }

    func unseal(sealed: Data, reason: String) throws -> Data {
        try core { try unsealDetailed(sealed, reason: reason) }
    }

    func indexKey() throws -> Data {
        try core { try indexKeyDetailed() }
    }

    func reset() throws {
        try core { try deleteCopyKeys() }
    }

    /// The core sees only `KeystoreError`. The `…Detailed` steps throw the
    /// raw failure, whose codes the check screen shows.
    private func core<T>(_ body: () throws -> T) throws -> T {
        do {
            return try body()
        } catch let failure as EnclaveFailure {
            throw failure.error
        }
    }

    #if DEBUG
    /// Finds or makes the seal key without sealing anything, for the check
    /// screen.
    @discardableResult
    func prepare() throws -> String {
        let group = try KeychainGroup.shared()
        try Self.preflightBiometry()
        let key = try lock.withLock { try findOrCreateKey(group: group) }
        return key.created ? "created" : "found"
    }
    #endif

    func sealDetailed(_ secret: Data) throws -> Data {
        let group = try KeychainGroup.shared()
        try Self.preflightBiometry()
        let key = try lock.withLock { try findOrCreateKey(group: group) }
        if !key.created, enrolmentChanged(keyLabel: key.label, group: group) {
            throw EnclaveFailure(.KeyGone, "Face ID enrolment changed since the seal key was made")
        }
        guard let publicKey = SecKeyCopyPublicKey(key.ref) else {
            throw EnclaveFailure.failed("the seal key has no public key")
        }
        let ciphertext = try Self.encrypt(secret, to: publicKey)
        guard key.label.count <= Int(UInt8.max) else {
            throw EnclaveFailure.failed("the seal key's label is too long")
        }
        return Data([Self.formatVersion, UInt8(key.label.count)]) + key.label + ciphertext
    }

    static func encrypt(_ plaintext: Data, to publicKey: SecKey) throws -> Data {
        guard SecKeyIsAlgorithmSupported(publicKey, .encrypt, algorithm) else {
            throw EnclaveFailure.failed("the seal key does not support ECIES")
        }
        var error: Unmanaged<CFError>?
        guard let ciphertext = SecKeyCreateEncryptedData(publicKey, algorithm, plaintext as CFData, &error) else {
            throw EnclaveFailure.from(error, while: "encrypting")
        }
        return ciphertext as Data
    }

    /// Sealing to a key nobody can open helps no one: refuse when Face ID
    /// cannot be used at all. A lockout is temporary, so it seals anyway.
    private static func preflightBiometry() throws {
        let context = LAContext()
        defer { context.invalidate() }
        var error: NSError?
        if context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error) { return }
        guard let error, error.domain == LAErrorDomain else { return }
        switch LAError.Code(rawValue: error.code) {
        case .biometryNotEnrolled?, .passcodeNotSet?:
            // Face ID cannot be enrolled without a passcode, so both mean
            // "Face ID is not set up".
            throw EnclaveFailure(.NotEnrolled, "LAError \(error.code)")
        case .biometryNotAvailable?:
            throw EnclaveFailure(.Failed(message: Self.notAvailableText(error.code)), "LAError \(error.code)")
        default:
            return
        }
    }

    func unsealDetailed(_ sealed: Data, reason: String) throws -> Data {
        // The core calls from its own threads. Blocking the main thread on
        // Face ID would freeze the app for as long as the prompt is up.
        guard !Thread.isMainThread else {
            assertionFailure("EnclaveKeystore.unseal blocks on Face ID: call it off the main thread")
            throw EnclaveFailure.failed("unseal was called on the main thread")
        }
        let group = try KeychainGroup.shared()
        let (label, ciphertext) = try Self.parse(sealed)

        // Before any prompt: a key that is gone, or whose faces changed,
        // can never open this.
        guard try findKey(label: label, group: group, context: nil) != nil else {
            throw EnclaveFailure(.KeyGone, "the seal key is not in the keychain")
        }
        if enrolmentChanged(keyLabel: label, group: group) {
            throw EnclaveFailure(.KeyGone, "Face ID enrolment changed since the seal key was made")
        }

        // One fresh context per call: no reuse of an earlier match and no
        // reuse of the device unlock.
        let context = LAContext()
        defer { context.invalidate() }
        context.localizedFallbackTitle = "" // hides "Enter Passcode"
        context.touchIDAuthenticationAllowableReuseDuration = 0
        if let code = Self.evaluateFaceId(context, reason: reason) {
            throw EnclaveFailure(Self.mapLocalAuthentication(code), "LAError \(code)")
        }

        // The matched context opens the key; it must not prompt again.
        context.interactionNotAllowed = true
        guard let key = try findKey(label: label, group: group, context: context) else {
            throw EnclaveFailure(.KeyGone, "the seal key vanished during Face ID")
        }
        var error: Unmanaged<CFError>?
        if let plaintext = SecKeyCreateDecryptedData(key, Self.algorithm, ciphertext as CFData, &error) {
            return plaintext as Data
        }
        let failure = EnclaveFailure.from(error, while: "decrypting")
        // Face ID matched, yet the Secure Enclave refused the key: the
        // faces it was bound to are gone. Ending every copy is the safe
        // answer; anything else would seal new copies to a dead key.
        let keyStillThere = (try? findKey(label: label, group: group, context: nil)).map { $0 != nil }
        if failure.isAuthRefusal || keyStillThere == false || enrolmentChanged(keyLabel: label, group: group) {
            throw EnclaveFailure(.KeyGone, "Face ID matched but the key refused: \(failure.detail)")
        }
        throw failure
    }

    private static func parse(_ sealed: Data) throws -> (label: Data, ciphertext: Data) {
        let bytes = [UInt8](sealed)
        guard bytes.count > 2, bytes[0] == formatVersion else {
            throw EnclaveFailure.failed("not sealed by this keystore")
        }
        let labelEnd = 2 + Int(bytes[1])
        // ECIES output is at least a 65-byte point and a 16-byte tag.
        guard bytes[1] > 0, bytes.count >= labelEnd + 65 + 16 else {
            throw EnclaveFailure.failed("not sealed by this keystore")
        }
        return (Data(bytes[2..<labelEnd]), Data(bytes[labelEnd...]))
    }

    /// Shows Face ID and waits for it. Returns nil on a match, otherwise
    /// the `LAError` code.
    private static func evaluateFaceId(_ context: LAContext, reason: String) -> Int? {
        let done = DispatchSemaphore(value: 0)
        let answer = Answer()
        context.evaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, localizedReason: reason) { ok, error in
            if !ok {
                let error = error as NSError?
                answer.code = error.map { $0.domain == LAErrorDomain ? $0.code : LAError.Code.invalidContext.rawValue }
                    ?? LAError.Code.invalidContext.rawValue
            }
            done.signal()
        }
        done.wait()
        return answer.code
    }

    private final class Answer: @unchecked Sendable {
        var code: Int? // written before `signal`, read after `wait`
    }

    /// How a Face ID answer reaches the core (design.md 2.5).
    static func mapLocalAuthentication(_ code: Int) -> KeystoreError {
        switch LAError.Code(rawValue: code) {
        case .userCancel?, .systemCancel?, .appCancel?, .userFallback?:
            // systemCancel: the app went to the background or another
            // prompt took over. userFallback cannot happen with the button
            // hidden; if it does, it is still not a passcode.
            return .Cancelled
        case .authenticationFailed?:
            return .NotRecognised
        case .biometryLockout?:
            return .LockedOut
        case .biometryNotEnrolled?:
            return .NotEnrolled
        case .passcodeNotSet?:
            // Removing the passcode deletes every WhenPasscodeSet item,
            // the seal key included.
            return .KeyGone
        case .biometryNotAvailable?:
            return .Failed(message: notAvailableText(code))
        default:
            return .Failed(message: "Face ID could not run (LAError \(code))")
        }
    }

    private static func notAvailableText(_ code: Int) -> String {
        "Face ID isn't available to paw. It may be turned off for paw in Settings > Face ID & Passcode > Other Apps. (LAError \(code))"
    }

    func indexKeyDetailed() throws -> Data {
        let group = try KeychainGroup.shared()
        return try lock.withLock {
            if let existing = try readSecret(account: Self.indexAccount, group: group) {
                guard existing.count == 32 else { throw EnclaveFailure.failed("the index key has the wrong size") }
                return existing
            }
            var bytes = [UInt8](repeating: 0, count: 32)
            defer { wipe(&bytes) }
            guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else {
                throw EnclaveFailure.failed("no random bytes for the index key")
            }
            var fresh = Data(bytes)
            defer { wipe(&fresh) }
            // No access control: it must open without a prompt. Unlocked
            // only, because the copy files it names are `Complete`
            // protected and unreadable while locked anyway.
            let status = addSecret(fresh, account: Self.indexAccount, group: group,
                                   accessible: kSecAttrAccessibleWhenUnlockedThisDeviceOnly)
            switch status {
            case errSecSuccess, errSecDuplicateItem:
                // Read back what the keychain holds: on a duplicate, the
                // other process won and both must use its key.
                guard let stored = try readSecret(account: Self.indexAccount, group: group), stored.count == 32 else {
                    throw EnclaveFailure.failed("the index key could not be read back")
                }
                return stored
            default:
                throw EnclaveFailure.status(status, while: "storing the index key")
            }
        }
    }

    /// Deletes the seal key, the index key and the enrolment records, so
    /// every copy sealed so far can never open again: Lock now, and
    /// removing the store.
    func deleteCopyKeys() throws {
        try deleteCopyKeys(group: try KeychainGroup.shared())
    }

    /// Deletes this namespace's keys in `group`: the shared group, or the
    /// app's own, where paw kept them before AutoFill (`LegacyStore`).
    func deleteCopyKeys(group: String) throws {
        try lock.withLock {
            var failures: [String] = []
            let keys: [String: Any] = [
                kSecClass as String: kSecClassKey,
                kSecAttrApplicationTag as String: sealTag,
                kSecAttrAccessGroup as String: group,
            ]
            // The other process's enrolment record stays: it names the key
            // it was written for, so it is ignored once that key is gone.
            for query in [keys, secretQuery(account: Self.indexAccount, group: group),
                          secretQuery(account: Self.enrolmentAccount, group: group),
                          secretQuery(account: Self.legacyEnrolmentAccount, group: group)] {
                let status = SecItemDelete(query as CFDictionary)
                if status != errSecSuccess && status != errSecItemNotFound {
                    failures.append("OSStatus \(status)")
                }
            }
            if !failures.isEmpty {
                throw EnclaveFailure.failed("could not delete the Face ID keys (\(failures.joined(separator: ", ")))")
            }
        }
    }

    // MARK: Seal key in the keychain

    private struct FoundKey {
        let ref: SecKey
        let label: Data
        let created: Bool
    }

    /// Call with `lock` held.
    private func findOrCreateKey(group: String) throws -> FoundKey {
        if let existing = try allKeys(group: group).min(by: { $0.label.lexicographicallyPrecedes($1.label) }) {
            return existing
        }
        var error: Unmanaged<CFError>?
        guard let access = SecAccessControlCreateWithFlags(
            kCFAllocatorDefault,
            kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly,
            [.privateKeyUsage, .biometryCurrentSet],
            &error
        ) else {
            throw EnclaveFailure.from(error, while: "making the access control")
        }
        let attributes: [String: Any] = [
            kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
            kSecAttrKeySizeInBits as String: 256,
            kSecAttrTokenID as String: kSecAttrTokenIDSecureEnclave,
            kSecAttrAccessGroup as String: group,
            kSecPrivateKeyAttrs as String: [
                kSecAttrIsPermanent as String: true,
                kSecAttrApplicationTag as String: sealTag,
                kSecAttrAccessGroup as String: group,
                kSecAttrAccessControl as String: access,
            ] as [String: Any],
        ]
        guard let key = SecKeyCreateRandomKey(attributes as CFDictionary, &error) else {
            throw EnclaveFailure.from(error, while: "making the seal key")
        }
        guard let label = (SecKeyCopyAttributes(key) as? [String: Any])?[kSecAttrApplicationLabel as String] as? Data else {
            throw EnclaveFailure.failed("the new seal key has no label")
        }
        // Remember which faces the key is bound to, so a change is caught
        // before a pointless prompt. If this fails, unseal still finds out
        // from the Secure Enclave's refusal.
        if let state = Self.currentEnrolment() {
            recordEnrolment(state, keyLabel: label, group: group)
        }
        return FoundKey(ref: key, label: label, created: true)
    }

    private func keyQuery(group: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassKey,
            kSecAttrKeyClass as String: kSecAttrKeyClassPrivate,
            kSecAttrApplicationTag as String: sealTag,
            kSecAttrTokenID as String: kSecAttrTokenIDSecureEnclave,
            kSecAttrAccessGroup as String: group,
        ]
    }

    /// Every seal key, normally one. Getting a reference never prompts; the
    /// non-interactive context makes sure of it.
    private func allKeys(group: String) throws -> [FoundKey] {
        let context = LAContext()
        defer { context.invalidate() }
        context.interactionNotAllowed = true
        var query = keyQuery(group: group)
        query[kSecMatchLimit as String] = kSecMatchLimitAll
        query[kSecReturnRef as String] = true
        query[kSecReturnAttributes as String] = true
        query[kSecUseAuthenticationContext as String] = context
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return [] }
        guard status == errSecSuccess else { throw EnclaveFailure.status(status, while: "finding the seal key") }
        guard let items = result as? [[String: Any]] else { return [] }
        return items.compactMap { item in
            guard let ref = asSecKey(item[kSecValueRef as String].map { $0 as AnyObject }),
                  let label = item[kSecAttrApplicationLabel as String] as? Data else { return nil }
            return FoundKey(ref: ref, label: label, created: false)
        }
    }

    private func findKey(label: Data, group: String, context: LAContext?) throws -> SecKey? {
        let probe = context ?? LAContext()
        defer { if context == nil { probe.invalidate() } }
        if context == nil { probe.interactionNotAllowed = true }
        var query = keyQuery(group: group)
        query[kSecAttrApplicationLabel as String] = label
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        query[kSecReturnRef as String] = true
        query[kSecUseAuthenticationContext as String] = probe
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw EnclaveFailure.status(status, while: "finding the seal key") }
        guard let key = asSecKey(result) else { throw EnclaveFailure.failed("the keychain returned something that is not a key") }
        return key
    }

    // MARK: Face ID enrolment

    /// The enrolled faces as Face ID reports them now, or nil if there are
    /// none or it cannot say. Changes whenever a face is enrolled or reset.
    static func currentEnrolment() -> Data? {
        let context = LAContext()
        defer { context.invalidate() }
        var error: NSError?
        _ = context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error)
        return context.domainState.biometry.stateHash
    }

    /// True only when this process recorded the faces for this very key
    /// and they differ now.
    ///
    /// A process that has no record for the key (the other process made
    /// it) records what it sees and answers false: the Secure Enclave
    /// still refuses a key whose faces changed, which unseal reports.
    private func enrolmentChanged(keyLabel: Data, group: String) -> Bool {
        guard let current = Self.currentEnrolment() else { return false }
        if let stored = try? readSecret(account: Self.enrolmentAccount, group: group),
           stored.count > keyLabel.count, stored.prefix(keyLabel.count) == keyLabel {
            return Data(stored.dropFirst(keyLabel.count)) != current
        }
        recordEnrolment(current, keyLabel: keyLabel, group: group)
        return false
    }

    /// The record is the key's label followed by the enrolment hash, so a
    /// record left from an earlier key is never compared.
    private func recordEnrolment(_ state: Data, keyLabel: Data, group: String) {
        _ = SecItemDelete(secretQuery(account: Self.enrolmentAccount, group: group) as CFDictionary)
        _ = addSecret(keyLabel + state, account: Self.enrolmentAccount, group: group,
                      accessible: kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly)
    }

    // MARK: Generic-password items

    private func secretQuery(account: String, group: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrAccessGroup as String: group,
            kSecAttrSynchronizable as String: false,
            kSecUseDataProtectionKeychain as String: true,
        ]
    }

    private func readSecret(account: String, group: String) throws -> Data? {
        var query = secretQuery(account: account, group: group)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw EnclaveFailure.status(status, while: "reading \(account)") }
        guard let data = result as? Data else { throw EnclaveFailure.failed("\(account) is not data") }
        return data
    }

    private func addSecret(_ value: Data, account: String, group: String, accessible: CFString) -> OSStatus {
        var query = secretQuery(account: account, group: group)
        query[kSecValueData as String] = value
        query[kSecAttrAccessible as String] = accessible
        return SecItemAdd(query as CFDictionary, nil)
    }
}

/// `shared()`: where every Face ID copy item lives, the App Group
/// `group.com.marktoda.paw`. An App Group is also a keychain access group
/// on iOS, and both paw and its AutoFill extension are signed with it, so a
/// copy made in one opens in the other. scripts/install.sh signs each
/// target with only its own application identifier as keychain group (it
/// drops the profile's wildcard), so `<team>.com.marktoda.paw` would not
/// reach the extension.
///
/// `resolve()`: `<team id>.com.marktoda.paw`, the app's own
/// application-identifier group, for items only the app may read. The team
/// id is not known at build time (CI builds unsigned), so it is read once
/// from the group the keychain gives an item stored without one: the first
/// group in the signed entitlements.
enum KeychainGroup {
    /// The one place the group is named.
    static let name = "com.marktoda.paw"

    /// The group shared with the AutoFill extension. In the unsigned
    /// simulator build there is no App Group, so it is the app's own.
    static func shared() throws -> String {
        #if targetEnvironment(simulator)
        return try resolve()
        #else
        return StoreLocation.appGroup
        #endif
    }

    static func resolve() throws -> String {
        switch resolved {
        case .success(let group): return group
        case .failure(let failure): throw failure
        }
    }

    private static let resolved: Result<String, EnclaveFailure> = {
        do {
            let defaultGroup = try defaultAccessGroup()
            let prefix = defaultGroup.prefix { $0 != "." }
            let isTeamId = prefix.count == 10 && prefix.allSatisfy { $0.isASCII && ($0.isUppercase || $0.isNumber) }
            if isTeamId { return .success("\(prefix).\(name)") }
            #if targetEnvironment(simulator)
            // An unsigned simulator build has no team: use what it has.
            return .success(defaultGroup)
            #else
            return .failure(.failed("the keychain group \"\(defaultGroup)\" has no team id"))
            #endif
        } catch let failure as EnclaveFailure {
            return .failure(failure)
        } catch {
            return .failure(.failed("could not read the keychain group"))
        }
    }()

    /// Stores (or finds) an empty marker item without a group and reads
    /// back the group the keychain put it in. The marker holds nothing.
    private static func defaultAccessGroup() throws -> String {
        let base: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "com.marktoda.paw.keychain-group",
            kSecAttrAccount as String: "marker",
            kSecAttrSynchronizable as String: false,
            kSecUseDataProtectionKeychain as String: true,
        ]
        var add = base
        add[kSecValueData as String] = Data()
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        add[kSecReturnAttributes as String] = true
        var result: CFTypeRef?
        var status = SecItemAdd(add as CFDictionary, &result)
        if status == errSecDuplicateItem {
            var query = base
            query[kSecReturnAttributes as String] = true
            query[kSecMatchLimit as String] = kSecMatchLimitOne
            result = nil
            status = SecItemCopyMatching(query as CFDictionary, &result)
        }
        guard status == errSecSuccess else { throw EnclaveFailure.status(status, while: "finding the keychain group") }
        guard let group = (result as? [String: Any])?[kSecAttrAccessGroup as String] as? String, !group.isEmpty else {
            throw EnclaveFailure.failed("the keychain reported no group")
        }
        return group
    }
}

/// A keystore failure: what the core is told, and the raw codes behind it
/// for the check screen. Never holds secret data.
struct EnclaveFailure: Error {
    let error: KeystoreError
    let detail: String
    /// Set when the failure came from a decrypt the key's access control
    /// refused, as opposed to bad input.
    var isAuthRefusal = false

    init(_ error: KeystoreError, _ detail: String) {
        self.error = error
        self.detail = detail
    }

    static func failed(_ message: String) -> EnclaveFailure {
        EnclaveFailure(.Failed(message: message), message)
    }

    static func status(_ status: OSStatus, while doing: String) -> EnclaveFailure {
        let text = SecCopyErrorMessageString(status, nil) as String? ?? ""
        let detail = "OSStatus \(status) while \(doing)\(text.isEmpty ? "" : ": \(text)")"
        if status == errSecMissingEntitlement {
            return EnclaveFailure(.Failed(message: "paw is not allowed to use its keychain group (\(detail))"), detail)
        }
        return EnclaveFailure(.Failed(message: detail), detail)
    }

    /// Takes ownership of a CFError from a Security call.
    static func from(_ error: Unmanaged<CFError>?, while doing: String) -> EnclaveFailure {
        guard let cfError = error?.takeRetainedValue() else {
            return failed("failed without an error while \(doing)")
        }
        let domain = CFErrorGetDomain(cfError) as String
        let code = CFErrorGetCode(cfError)
        let detail = "\(domain) \(code) while \(doing)"
        var failure: EnclaveFailure
        switch (domain, code) {
        case (LAErrorDomain, _):
            failure = EnclaveFailure(EnclaveKeystore.mapLocalAuthentication(code), detail)
        case (NSOSStatusErrorDomain, Int(errSecUserCanceled)), ("CryptoTokenKit", -4):
            failure = EnclaveFailure(.Cancelled, detail)
        default:
            failure = EnclaveFailure(.Failed(message: detail), detail)
        }
        // errSecAuthFailed, errSecItemNotFound, errSecInteractionNotAllowed;
        // TKError authenticationFailed, objectNotFound, authenticationNeeded.
        let osRefusals = [errSecAuthFailed, errSecItemNotFound, errSecInteractionNotAllowed].map { Int($0) }
        failure.isAuthRefusal = (domain == NSOSStatusErrorDomain && osRefusals.contains(code))
            || (domain == "CryptoTokenKit" && [-5, -6, -9].contains(code))
            || (domain == LAErrorDomain && code != LAError.Code.userCancel.rawValue
                && code != LAError.Code.systemCancel.rawValue && code != LAError.Code.appCancel.rawValue)
        return failure
    }
}

private func asSecKey(_ object: AnyObject?) -> SecKey? {
    guard let object, CFGetTypeID(object) == SecKeyGetTypeID() else { return nil }
    return unsafeBitCast(object, to: SecKey.self)
}

/// Zeroes a buffer this code owns. Only meaningful for a buffer nothing
/// else shares: a `Data` or array copied elsewhere keeps its own bytes.
func wipe(_ bytes: inout [UInt8]) {
    bytes.withUnsafeMutableBytes { buffer in
        if let base = buffer.baseAddress { _ = memset_s(base, buffer.count, 0, buffer.count) }
    }
}

func wipe(_ data: inout Data) {
    let count = data.count
    data.withUnsafeMutableBytes { buffer in
        if let base = buffer.baseAddress { _ = memset_s(base, count, 0, count) }
    }
}
