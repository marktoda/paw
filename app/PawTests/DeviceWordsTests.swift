import XCTest
@testable import Paw

/// Copy is written for an iPhone with Face ID; `Device.words` says it in
/// an iPad's or a Touch ID device's words.
final class DeviceWordsTests: XCTestCase {
    func testAnIPhoneWithFaceIdReadsAsWritten() {
        let text = "Face ID is off until you enter your iPhone passcode"
        XCTAssertEqual(Device.words(text, name: "iPhone", biometrics: "Face ID"), text)
    }

    func testAnIPadSaysIPad() {
        XCTAssertEqual(
            Device.words("Your change is saved on this iPhone.", name: "iPad", biometrics: "Face ID"),
            "Your change is saved on this iPad.")
        XCTAssertEqual(
            Device.words("This iPhone's store has changes paw didn't make", name: "iPad", biometrics: "Face ID"),
            "This iPad's store has changes paw didn't make")
    }

    func testATouchIdIPadSaysBoth() {
        XCTAssertEqual(
            Device.words("Face ID is locked. Unlock your iPhone with its passcode, then try again.",
                         name: "iPad", biometrics: "Touch ID"),
            "Touch ID is locked. Unlock your iPad with its passcode, then try again.")
    }

    func testSayingItTwiceChangesNothing() {
        let once = Device.words("Face ID changed on this iPhone", name: "iPad", biometrics: "Touch ID")
        XCTAssertEqual(Device.words(once, name: "iPad", biometrics: "Touch ID"), once)
    }
}
