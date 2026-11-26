import Foundation

#if DEBUG && targetEnvironment(simulator)
/// "Let the next open finish after Cancel": the next task the sheet follows
/// that finishes ends as if Cancel had been tapped just as the card
/// answered, past the point where the core can stop it. The sheet closes,
/// and the task's end goes where a late end goes. The simulated key cannot
/// be cancelled that late by itself.
@MainActor
enum SimulatedLateCancel {
    static var armed = false

    /// `job`, with its next done end turned into a late one while armed.
    static func applied<Step>(to job: KeyJob<Step>) -> KeyJob<Step> {
        var job = job
        let ended = job.ended
        let letGo = job.letGo
        job.ended = { end in
            guard armed, case .done = end else { return ended(end) }
            armed = false
            letGo(end, false)
            return nil
        }
        return job
    }
}
#endif
