import XCTest
@testable import Paw

/// The suggestions paw publishes for a store: one per entry whose path
/// names a site, and one per domain a name-only entry has filled on.
final class CredentialIdentitiesTests: XCTestCase {
    func testSitePathsAndPairedNamesGiveSuggestionsAndNothingElseDoes() throws {
        let stateDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("paw-unit-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: stateDir) }
        let repo = URL(fileURLWithPath: storeRepoDir(stateDir: stateDir.path), isDirectory: true)
        // Only names matter: the files are never opened.
        for path in ["web/github.com", "github.com/alice", "web/GitHub", "sites/co.uk", "alaskaair", "bank/bankofamerica"] {
            let file = repo.appendingPathComponent(path + ".gpg")
            try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data("not encrypted".utf8).write(to: file)
        }
        try Data("0000\n".utf8).write(to: repo.appendingPathComponent(".gpg-id"))
        let store = try PassStore(dir: repo.path)
        let pairings = SitePairings.open(stateDir: stateDir.path)
        XCTAssertTrue(try pairings.recordPairing(store: store, entry: "alaskaair", service: "https://www.alaskaair.com"))

        let made = CredentialIdentities.make(from: store, pairings: pairings).map { "\($0.path) \($0.domain) \($0.user)" }
        XCTAssertEqual(made, [
            "alaskaair alaskaair.com alaskaair",
            "github.com/alice github.com alice",
            "web/github.com github.com github.com",
        ])
    }
}
