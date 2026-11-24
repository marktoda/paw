import Darwin
import Foundation
import LocalAuthentication

/// The device's own words, so iPad copy says "iPad" and a Touch ID iPad says
/// "Touch ID". Strings are written for the iPhone ("this iPhone", "Face ID")
/// and pass through `Device.words` where they are shown; text from the core
/// goes through it too. Nothing here is main-actor bound, so any code can
/// use it.
enum Device {
    /// The hardware model, e.g. "iPhone17,1" or "iPad13,18". The simulator
    /// reports the model it simulates.
    private static let model: String = {
        if let simulated = ProcessInfo.processInfo.environment["SIMULATOR_MODEL_IDENTIFIER"] {
            return simulated
        }
        var size = 0
        guard sysctlbyname("hw.machine", nil, &size, nil, 0) == 0, size > 0 else { return "" }
        var bytes = [CChar](repeating: 0, count: size)
        guard sysctlbyname("hw.machine", &bytes, &size, nil, 0) == 0 else { return "" }
        return String(decoding: bytes.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
    }()

    /// "iPhone" or "iPad".
    static let name: String = model.hasPrefix("iPad") ? "iPad" : "iPhone"

    /// "Face ID" or "Touch ID", from the hardware; "Face ID" when there is
    /// neither, since paw's copies are then off anyway.
    static let biometrics: String = {
        let context = LAContext()
        // Sets `biometryType` even when nothing is enrolled.
        _ = context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil)
        return context.biometryType == .touchID ? "Touch ID" : "Face ID"
    }()

    /// `text` written for an iPhone with Face ID, in this device's words.
    static func words(_ text: String) -> String {
        var text = text
        if name != "iPhone" {
            text = text.replacingOccurrences(of: "iPhone", with: name)
        }
        if biometrics != "Face ID" {
            text = text.replacingOccurrences(of: "Face ID", with: biometrics)
        }
        return text
    }
}
