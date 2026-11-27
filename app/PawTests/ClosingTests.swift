import SwiftUI
import UIKit
import XCTest
@testable import Paw

/// When an open entry closes (design 2.5), on the model: the iPhone
/// locking, its Face ID copy ending, 2 minutes away, "Lock this password",
/// going back, and an open that finishes after Cancel.
@MainActor
final class ClosingTests: XCTestCase {
    /// Replaces the UI test `LifecycleTests.testAnOpenEntryClosesWhenTheIPhoneLocks`.
    func testAnOpenEntryClosesWhenTheIPhoneLocks() async throws {
        let fixture = try Fixture()
        let model = fixture.model
        let opened = await fixture.open("web/github.com")
        let id = try XCTUnwrap(opened)

        // What the system says as the iPhone locks (and the simulator's
        // "Lock the iPhone" control says the same).
        NotificationCenter.default.post(
            name: UIApplication.protectedDataWillBecomeUnavailableNotification, object: nil)
        await waitFor("the entry to close") { model.closedEntry(id) != nil }
        XCTAssertEqual(model.closedEntry(id)?.reason, .deviceLocked)
        XCTAssertEqual(model.closedEntry(id)?.ref.path, "web/github.com")
        XCTAssertNil(model.session(id), "A closed entry keeps nothing of it")
        XCTAssertEqual(model.path, [.entry(id)], "The screen stays, saying why")
    }

    /// Replaces the UI test `LifecycleTests.testAnOpenEntryClosesWhenItsFaceIdTimeEnds`.
    func testAnOpenEntryClosesWhenItsFaceIdTimeEndsAndOpensAgainWithTheKey() async throws {
        let fixture = try Fixture(touch: true)
        let model = fixture.model
        let opened = await fixture.open("web/github.com")
        let id = try XCTUnwrap(opened)
        XCTAssertEqual(model.session(id)?.hadCopy, true)

        fixture.clock.advanceMs(ms: 9 * 3_600_000)
        model.timePassed()
        XCTAssertEqual(model.closedEntry(id)?.reason, .copyEnded)
        XCTAssertNil(model.session(id), "A closed entry keeps nothing of it")
        XCTAssertNil(model.copies.copy(of: "web/github.com"))

        // Opening again needs the key; the PIN is still held.
        model.reopen(id)
        XCTAssertNil(model.closedEntry(id))
        XCTAssertNotNil(model.keyRequest)
        let noPin = await fixture.touchWhenAsked()
        XCTAssertTrue(noPin, "The PIN is still held")
        await fixture.dismissDoneSheet()
        let again = try XCTUnwrap(fixture.shownEntry)
        XCTAssertNotEqual(again, id)
        XCTAssertEqual(model.session(again)?.ref.path, "web/github.com")
        XCTAssertEqual(model.path, [.entry(again)])
    }

    func testAnOpenEntryStaysUnderTwoMinutesAway() async throws {
        let fixture = try Fixture()
        let model = fixture.model
        let opened = await fixture.open("web/github.com")
        let id = try XCTUnwrap(opened)
        model.scenePhaseChanged(.background)
        fixture.clock.advanceMs(ms: 60 * 1000)
        model.scenePhaseChanged(.active)
        XCTAssertNotNil(model.session(id))
        XCTAssertNil(model.closedEntry(id))
    }

    func testAnOpenEntryClosesAfterTwoMinutesAwayAndReopensWithFaceId() async throws {
        let fixture = try Fixture()
        let model = fixture.model
        let opened = await fixture.open("web/github.com")
        let id = try XCTUnwrap(opened)
        model.scenePhaseChanged(.background)
        fixture.clock.advanceMs(ms: 3 * 60 * 1000)
        model.scenePhaseChanged(.active)
        XCTAssertEqual(model.closedEntry(id)?.reason, .away)
        XCTAssertNil(model.session(id))

        // It still has its Face ID copy: no key sheet.
        model.reopen(id)
        XCTAssertNil(model.keyRequest)
        await waitFor("Face ID to open it") { fixture.shownEntry != nil }
        let again = try XCTUnwrap(fixture.shownEntry)
        XCTAssertNotEqual(again, id)
        XCTAssertEqual(model.session(again)?.ref.path, "web/github.com")
    }

    func testLockThisPasswordEndsOnlyItsOwnCopy() async throws {
        let fixture = try Fixture()
        let model = fixture.model
        await fixture.open("web/github.com")
        let opened = await fixture.open("web/gitlab.com", askingForPin: false)
        let id = try XCTUnwrap(opened)
        let session = try XCTUnwrap(model.session(id))

        model.lockEntry(session)
        XCTAssertEqual(model.toast?.text, "Locked. gitlab.com needs your key again.")
        XCTAssertNil(model.copies.copy(of: "web/gitlab.com"))
        XCTAssertNotNil(model.copies.copy(of: "web/github.com"))
        XCTAssertEqual(model.rowAccess("web/gitlab.com"), .needsKey)
        XCTAssertEqual(model.rowAccess("web/github.com"), .faceID)
        XCTAssertFalse(model.path.contains(.entry(id)), "Back to the list")
    }

    func testGoingBackClosesTheEntry() async throws {
        let fixture = try Fixture()
        let model = fixture.model
        let opened = await fixture.open("web/github.com")
        let id = try XCTUnwrap(opened)
        fixture.goHome()
        XCTAssertNil(model.session(id))
        XCTAssertNil(model.closedEntry(id), "Gone, not shown as closed")
    }

    /// Replaces the UI test `LifecycleTests.testAnOpenThatFinishesAfterCancelRemovesOnlyItsOwnCopy`.
    func testAnOpenThatFinishesAfterCancelRemovesOnlyItsOwnCopy() async throws {
        let fixture = try Fixture()
        let model = fixture.model
        await fixture.open("web/github.com")
        fixture.goHome()

        // The next open finishes past the point where Cancel can stop it,
        // as if Cancel came just as the card answered.
        fixture.keys.finishesNextAfterCancel = true
        model.open(EntryRef(path: "web/gitlab.com"))
        await waitFor("the open to end") { model.keyTask.pendingTasks == 0 }
        XCTAssertNil(model.keyRequest)
        model.keySheetDismissed()
        XCTAssertEqual(model.path, [], "A late finish after Cancel opens nothing")
        XCTAssertNotNil(model.copies.copy(of: "web/github.com"))
        XCTAssertNil(model.copies.copy(of: "web/gitlab.com"), "The late open's copy was removed")
        XCTAssertFalse(model.recentPaths.contains("web/gitlab.com"))
    }
}
