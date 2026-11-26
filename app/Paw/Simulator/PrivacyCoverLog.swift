import Observation

#if DEBUG && targetEnvironment(simulator)
/// For the UI tests: the cover is up only while paw is not active, when a
/// test can't look at it, so it counts each time it is drawn.
@MainActor @Observable
final class PrivacyCoverLog {
    static let shared = PrivacyCoverLog()
    var shown = 0
}
#endif
