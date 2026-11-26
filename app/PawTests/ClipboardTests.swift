import UIKit
import UniformTypeIdentifiers
import XCTest
@testable import Paw

/// What a copy puts on the pasteboard and when paw takes it off again, on
/// a pasteboard of the test's own.
@MainActor
final class ClipboardTests: XCTestCase {
    private final class TestPasteboard: Pasteboard {
        private(set) var changeCount = 0
        private(set) var options: [UIPasteboard.OptionsKey: Any] = [:]
        var items: [[String: Any]] = [] {
            didSet { changeCount += 1 }
        }

        func setItems(_ items: [[String: Any]], options: [UIPasteboard.OptionsKey: Any]) {
            self.options = options
            self.items = items
        }

        var text: String? {
            items.first?[UTType.utf8PlainText.identifier] as? String
        }
    }

    func testACopyStaysOnThisDeviceAndTheSystemEndsItAfter45Seconds() throws {
        let pasteboard = TestPasteboard()
        let clipboard = Clipboard(clock: SimClock(wallMs: 1_790_000_000_000).asClock(), pasteboard: pasteboard)
        let copied = Date()
        clipboard.copy("tidal-Quartz-88-ferns")

        XCTAssertEqual(pasteboard.text, "tidal-Quartz-88-ferns")
        XCTAssertEqual(pasteboard.options[.localOnly] as? Bool, true, "Never handed to Universal Clipboard")
        let expiry = try XCTUnwrap(pasteboard.options[.expirationDate] as? Date)
        XCTAssertEqual(expiry.timeIntervalSince(copied), 45, accuracy: 5)
    }

    func testPawTakesItsCopyOffAfter45SecondsOnItsClock() {
        let clock = SimClock(wallMs: 1_790_000_000_000)
        let pasteboard = TestPasteboard()
        let clipboard = Clipboard(clock: clock.asClock(), pasteboard: pasteboard)
        clipboard.copy("tidal-Quartz-88-ferns")

        clock.advanceMs(ms: 44_000)
        clipboard.clearIfExpired()
        XCTAssertEqual(pasteboard.text, "tidal-Quartz-88-ferns")

        clock.advanceMs(ms: 1_000)
        clipboard.clearIfExpired()
        XCTAssertNil(pasteboard.text)
    }

    func testWhatSomethingElseCopiedSinceIsLeftAlone() {
        let clock = SimClock(wallMs: 1_790_000_000_000)
        let pasteboard = TestPasteboard()
        let clipboard = Clipboard(clock: clock.asClock(), pasteboard: pasteboard)
        clipboard.copy("tidal-Quartz-88-ferns")
        pasteboard.setItems([[UTType.utf8PlainText.identifier: "a link"]], options: [:])

        clipboard.clearIfOurs()
        XCTAssertEqual(pasteboard.text, "a link")
        clock.advanceMs(ms: 60_000)
        clipboard.clearIfExpired()
        XCTAssertEqual(pasteboard.text, "a link")
    }
}
