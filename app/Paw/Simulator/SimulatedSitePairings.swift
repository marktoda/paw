import Foundation

#if DEBUG && targetEnvironment(simulator)
/// `-pawSimPairings 1`: the fixture's name-only passwords have filled on a
/// site each, so Sites used has rows for the UI tests. Once per launch.
@MainActor
enum SimulatedSitePairings {
    private static var seeded = false

    static func seedOnce(_ pairings: SitePairings, store: PassStore) {
        guard LaunchArgument.simPairings.bool, !seeded else { return }
        seeded = true
        _ = try? pairings.recordPairing(store: store, entry: "email/fastmail", service: "https://www.fastmail.com")
        _ = try? pairings.recordPairing(store: store, entry: "work/Jira", service: "https://jira.com")
    }
}
#endif
