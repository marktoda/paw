import Foundation

/// Where paw kept the store before AutoFill moved it to the App Group:
/// `Application Support/paw/` (the store and the Face ID copies' files) and
/// the copies' keychain items in the app's own keychain group. Nothing is
/// moved over: the store is downloaded again at setup, and a copy is made
/// again the next time the key opens an entry. This deletes what is left,
/// at launch and with "Remove the store". The app only; never the
/// extension.
enum LegacyStore {
    private static let queue = DispatchQueue(label: "paw-legacy-store", qos: .utility)

    /// Deletes the old folder and the old keychain items off the main
    /// thread, then says whether the folder is still there. Safe to run
    /// any number of times.
    static func remove(then done: (@MainActor @Sendable (_ folderLeft: Bool) -> Void)? = nil) {
        queue.async {
            let left = removeNow()
            guard let done else { return }
            DispatchQueue.main.async {
                MainActor.assumeIsolated { done(left) }
            }
        }
    }

    /// `Application Support/paw/`, unless that is where the store is now.
    static func folder() -> URL? {
        guard let support = try? FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: false)
        else { return nil }
        let old = support.appendingPathComponent("paw", isDirectory: true)
        guard let now = try? StoreLocation.standard().root,
              now.standardizedFileURL != old.standardizedFileURL else { return nil }
        return old
    }

    /// Returns whether the old folder is still there.
    private static func removeNow() -> Bool {
        let manager = FileManager.default
        var left = false
        if let folder = folder(), manager.fileExists(atPath: folder.path) {
            do {
                try manager.removeItem(at: folder)
            } catch {
                left = true
            }
        }
        #if !targetEnvironment(simulator)
        // The old seal key, index key and enrolment marker. Only items in
        // the copies' namespace: the GitHub sign-in in the same group stays.
        if let old = try? KeychainGroup.resolve(), old != (try? KeychainGroup.shared()) {
            try? EnclaveKeystore().resetDetailed(group: old)
        }
        #endif
        return left
    }

    #if DEBUG && targetEnvironment(simulator)
    /// `-pawPlantLegacy 1`: a store left where paw kept it before, for the
    /// test that launch deletes it. That launch deletes nothing.
    static var plantsInstead: Bool {
        LaunchArgument.plantLegacy.bool
    }

    static func plant(then done: @escaping @MainActor @Sendable (_ folderLeft: Bool) -> Void) {
        queue.async {
            var left = false
            if let folder = folder() {
                let copies = folder.appendingPathComponent("face-id", isDirectory: true)
                let state = folder.appendingPathComponent("store", isDirectory: true)
                try? FileManager.default.createDirectory(at: copies, withIntermediateDirectories: true)
                try? FileManager.default.createDirectory(at: state, withIntermediateDirectories: true)
                try? Data("step=done\n".utf8).write(to: state.appendingPathComponent("setup"))
                try? Data("old copy".utf8).write(to: copies.appendingPathComponent("web-github.com"))
                left = FileManager.default.fileExists(atPath: folder.path)
            }
            DispatchQueue.main.async {
                MainActor.assumeIsolated { done(left) }
            }
        }
    }
    #endif
}
