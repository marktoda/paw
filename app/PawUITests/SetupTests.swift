import XCTest

/// First-run setup against the simulated key and the core's simulated git
/// host, running inside the app.
final class SetupTests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
        app = XCUIApplication()
    }

    func testTheSimulatedHostServesACloneInsideTheSimulator() {
        app.launchArguments = ["-pawSimHostProbe", "1", "-pawSimPin", "123456", "-pawSimTouch", "0"]
        app.launch()
        let result = app.staticTexts["probe-result"]
        XCTAssertTrue(result.waitForExistence(timeout: 10))
        let done = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "NOT (label BEGINSWITH 'Running')"), object: result)
        _ = XCTWaiter().wait(for: [done], timeout: 90)
        let attachment = XCTAttachment(string: result.label)
        attachment.lifetime = .keepAlways
        add(attachment)
        XCTAssertTrue(result.label.hasPrefix("PROBE OK"), result.label)
    }
}
