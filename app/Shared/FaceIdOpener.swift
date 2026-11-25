import Foundation
import Observation

/// Opens an entry from its Face ID copy, for the app and the AutoFill
/// extension: one at a time, with Face ID's prompt off the main thread.
@MainActor @Observable
final class FaceIdOpener {
    /// How long an entry opened with the key stays available to Face ID.
    static let windowSecs: UInt32 = 8 * 60 * 60

    /// The entry Face ID is being asked for, to ignore a second tap.
    private(set) var opening: String?
    @ObservationIgnored private var generation = 0
    @ObservationIgnored private let queue = DispatchQueue(label: "paw-bio", qos: .userInitiated)

    /// Asks Face ID for `entry` and hands `finished` what it opened. Does
    /// nothing while another is being asked for, a forgotten one included.
    /// `reason` is what the prompt says it is for. `answered` runs on every
    /// answer, a forgotten one too, before `finished`.
    func open(_ entry: EntryRef, store: PassStore, copies: CopyCache, reason: String,
              answered: (@MainActor () -> Void)? = nil,
              finished: @escaping @MainActor (Result<FaceIdOpen, Error>) -> Void) {
        guard opening == nil else { return }
        opening = entry.path
        let started = generation
        let reply = Handoff(value: (answered, finished))
        let path = entry.path
        queue.async {
            let result = Result { try store.openEntryWithFaceId(copies: copies, entry: path, reason: reason) }
            let handoff = Handoff(value: result)
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    self?.opening = nil
                    reply.value.0?()
                    guard let self, started == self.generation else {
                        // Forgotten while Face ID was up: nothing opens.
                        if case .success(.opened(let opened)) = handoff.value { opened.close() }
                        return
                    }
                    reply.value.1(handoff.value)
                }
            }
        }
    }

    /// What is being opened now is closed when it arrives, unseen: Lock now,
    /// or the request ended. Another open waits until it has arrived.
    func forget() {
        generation += 1
    }
}
