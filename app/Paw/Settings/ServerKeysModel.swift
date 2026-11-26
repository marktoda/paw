import Foundation
import Observation

/// The store's server keys for Settings: the ones paw trusts, a check of
/// the one the server presents now, forgetting and trusting. Forgetting
/// and trusting take the store lock, so they run inside background time
/// (`BackgroundTime`).
@MainActor @Observable
final class ServerKeysModel {
    /// What paw trusts for the server; `nil` while it can't be read.
    private(set) var keys: ServerKeys?
    /// The key the server presented, waiting for Trust.
    private(set) var presented: ServerKey?
    private(set) var checking = false
    private(set) var problem: (title: String, detail: String)?

    @ObservationIgnored private let sync: StoreSync
    /// Server keys forgotten since the store was opened, by host: so the
    /// next check can say whether the key the server presents now is the
    /// one the person trusted before. Memory only.
    private var forgotten: [String: Set<String>] = [:]
    private let workQueue = DispatchQueue(label: "paw-server-keys", qos: .userInitiated)

    init(sync: StoreSync) {
        self.sync = sync
        read()
    }

    var host: String {
        keys?.host ?? ""
    }

    /// Keys were forgotten for this host before the one presented now.
    var forgottenBefore: Bool {
        !(forgotten[host] ?? []).isEmpty
    }

    /// The key presented now is not one of those forgotten.
    var presentedDiffers: Bool {
        guard let presented, let before = forgotten[host], !before.isEmpty else { return false }
        return !before.contains(presented.fingerprint)
    }

    func read() {
        do {
            keys = try sync.serverKeys()
        } catch {
            problem = ("Couldn't read the server key", describe(error))
        }
    }

    func forget() {
        let host = host
        let pinned = keys?.keys ?? []
        do {
            try BackgroundTime.run("Server key") { try sync.forgetHostKey() }
            forgotten[host, default: []].formUnion(pinned.map(\.fingerprint))
            problem = nil
        } catch {
            problem = ("The server key wasn't forgotten", describe(error))
        }
        read()
    }

    /// Connects without the key and without logging in: network only.
    func check() {
        checking = true
        problem = nil
        let sync = sync
        workQueue.async { [weak self] in
            let result = Handoff(value: Result { try sync.checkServerKey() })
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.checked(result.value)
                }
            }
        }
    }

    private func checked(_ result: Result<ServerKey, Error>) {
        checking = false
        switch result {
        case .success(let key):
            presented = key
        case .failure(let error):
            problem = ("Couldn't reach the server", describe(error))
        }
    }

    func trust(_ key: ServerKey) {
        let host = host
        let sync = sync
        let time = BackgroundTime.begin("Server key")
        workQueue.async { [weak self] in
            let result = Handoff(value: Result { try sync.trustServerKey(fingerprint: key.fingerprint) })
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    time.end()
                    self?.trusted(result.value, host: host)
                }
            }
        }
    }

    private func trusted(_ result: Result<Void, Error>, host: String) {
        presented = nil
        if case .failure(let error) = result {
            problem = ("The server key wasn't trusted", describe(error))
        } else {
            forgotten[host] = nil
            problem = nil
        }
        read()
    }

    /// Cancel under the key presented.
    func dropPresented() {
        presented = nil
    }
}
