import AuthenticationServices
import XCTest
@testable import Paw

/// The provider's requests as iOS makes them, through the code the
/// extension's controller runs: what each request asks for, and the one
/// answer each gets.
@MainActor
final class AutoFillProviderTests: XCTestCase {
    /// What would reach iOS, recorded.
    @MainActor
    private final class Answers: AutoFillAnswers {
        var fills: [AutoFillCredential] = []
        var ends: [AutoFillEnd] = []

        func complete(with credential: AutoFillCredential) {
            fills.append(credential)
        }

        func cancel(_ reason: AutoFillEnd) {
            ends.append(reason)
        }
    }

    /// The provider holds its answers weakly: the test keeps them.
    private func makeProvider(_ opening: AutoFillOpening, answering answers: Answers) -> AutoFillProvider {
        let provider = AutoFillProvider(open: { opening })
        provider.answers = answers
        return provider
    }

    private func site(_ identifier: String) -> ASCredentialServiceIdentifier {
        ASCredentialServiceIdentifier(identifier: identifier, type: .URL)
    }

    private func passwordSuggestion(path: String?, site: String = "github.com") -> ASCredentialRequest {
        ASPasswordCredentialRequest(credentialIdentity: ASPasswordCredentialIdentity(
            serviceIdentifier: ASCredentialServiceIdentifier(identifier: site, type: .domain),
            user: "github.com", recordIdentifier: path))
    }

    // MARK: What iOS asks

    func testASuggestionNamesItsEntryItsSiteAndWhatItFills() {
        let password = AutoFillProvider.entry(of: passwordSuggestion(path: "web/github.com"))
        XCTAssertEqual(password?.request, .entry(path: "web/github.com", service: "github.com"))
        XCTAssertEqual(password?.kind, .password)

        let code = AutoFillProvider.entry(of: ASOneTimeCodeCredentialRequest(
            credentialIdentity: ASOneTimeCodeCredentialIdentity(
                serviceIdentifier: ASCredentialServiceIdentifier(identifier: "github.com", type: .domain),
                label: "github.com", recordIdentifier: "web/github.com")))
        XCTAssertEqual(code?.request, .entry(path: "web/github.com", service: "github.com"))
        XCTAssertEqual(code?.kind, .oneTimeCode)
    }

    func testASuggestionWithNoSiteIsCheckedAgainstNone() {
        let asked = AutoFillProvider.entry(of: passwordSuggestion(path: "web/github.com", site: ""))
        XCTAssertEqual(asked?.request, .entry(path: "web/github.com", service: nil))
    }

    func testASuggestionThatNamesNoEntryIsNotPaws() {
        for path in [nil, ""] {
            let request = passwordSuggestion(path: path)
            XCTAssertNil(AutoFillProvider.entry(of: request), "\(path ?? "nil")")
            XCTAssertEqual(AutoFillProvider.answerWithoutScreen(request), .credentialIdentityNotFound)
        }
        XCTAssertEqual(
            AutoFillProvider.answerWithoutScreen(passwordSuggestion(path: "web/github.com")), .userInteractionRequired,
            "Every fill needs Face ID or the key, so a suggestion always asks for a screen")
    }

    func testAListIsForTheSitesAsIOSGivesThem() {
        let answers = Answers()
        let provider = makeProvider(.noStore, answering: answers)
        let model = provider.list([site("https://github.com/login"), site("github.com")], kind: .oneTimeCode)
        XCTAssertEqual(model.request, .list(services: ["https://github.com/login", "github.com"]))
        XCTAssertEqual(model.kind, .oneTimeCode)
        XCTAssertTrue(provider.model === model)
    }

    // MARK: One answer per request

    func testASuggestionThatIsNotPawsEndsOnce() {
        let answers = Answers()
        let provider = makeProvider(.noStore, answering: answers)
        XCTAssertNil(provider.entry(passwordSuggestion(path: "")))
        provider.viewWentAway()
        provider.shutDown()
        XCTAssertEqual(answers.ends, [.credentialIdentityNotFound])
        XCTAssertNil(provider.model)
    }

    func testCloseThenTheViewGoingAwayEndTheRequestOnce() async {
        let answers = Answers()
        let provider = makeProvider(.noStore, answering: answers)
        let model = provider.list([site("https://github.com")], kind: .password)
        model.start()
        await waitFor("no store, it is \(model.screen)") { model.screen == .noStore }
        model.cancel()
        model.cancel()
        provider.viewWentAway()
        provider.shutDown()
        XCTAssertEqual(answers.ends, [.userCanceled])
        XCTAssertTrue(answers.fills.isEmpty)
        XCTAssertNil(provider.model)
    }

    func testAFillWithTheKeyIsTheRequestsOnlyAnswer() async throws {
        let location = StoreLocation(root: FileManager.default.temporaryDirectory
            .appendingPathComponent("paw-unit-\(UUID().uuidString)", isDirectory: true))
        try FileManager.default.createDirectory(at: location.stateDir, withIntermediateDirectories: true)
        try writeFixtureStore(dir: location.repoDir.path)
        let key = SimulatedKey(pin: Fixture.pin, touchRequired: false, touchTimeoutMs: 15_000)
        let clock = SimClock(wallMs: Int64(Date().timeIntervalSince1970 * 1000)).asClock()
        let source = try AutoFillSource.open(
            at: location, clock: clock, connect: { key.asConnection() }, copies: { _ in nil })
        let answers = Answers()
        let provider = makeProvider(.ready(source), answering: answers)

        let model = provider.list([site("https://github.com/login")], kind: .password)
        model.start()
        await waitFor("the list, it is \(model.screen)") { model.screen == .list }
        let github = try XCTUnwrap(model.listedMatches.first { $0.path == "web/github.com" })
        XCTAssertTrue(model.opensWithKey(github), "With no Face ID copies every fill takes the key")
        model.pick(github)
        await waitFor("the PIN, the sheet is at \(model.keyTask.stage)") { model.keyTask.stage.asksForPin }
        model.keyTask.pin = Fixture.pin
        model.keyTask.continueWithPin()
        await waitFor("the fill") { !answers.fills.isEmpty }

        model.cancel()
        provider.viewWentAway()
        provider.shutDown()
        XCTAssertEqual(answers.fills, [.password(user: "you@example.com", password: Fixture.githubPassword)])
        XCTAssertEqual(answers.ends, [])
        XCTAssertNil(provider.model)
        XCTAssertNotEqual(source.keyPort.session()?.holdsPin(), true, "The PIN is forgotten once the request ends")
    }
}
