import Foundation

#if DEBUG && targetEnvironment(simulator)
/// `-pawSimPairings 1`: the fixture's name-only passwords have filled on a
/// site each, so Sites used has rows for the UI tests.
enum SimulatedSitePairings {
    static func seed(stateDir: String) {
        let pairings = SitePairings.open(stateDir: stateDir)
        guard let store = try? PassStore(dir: storeRepoDir(stateDir: stateDir)) else { return }
        _ = try? pairings.recordPairing(store: store, entry: "email/fastmail", service: "https://www.fastmail.com")
        _ = try? pairings.recordPairing(store: store, entry: "work/Jira", service: "https://jira.com")
    }
}
#endif
