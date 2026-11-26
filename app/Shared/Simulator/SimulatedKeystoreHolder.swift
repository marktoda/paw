import Foundation

#if DEBUG && targetEnvironment(simulator)
/// One simulated Face ID keystore per launch, for the app's Face ID copies
/// (`PlatformCopies`), the fixture's and the AutoFill harness's.
/// `-pawSimFaceId ok|cancel|nomatch|lockout|notenrolled` says how Face ID
/// answers.
enum SimulatedKeystoreHolder {
    static let keystore: SimKeystore = {
        let keystore = SimKeystore()
        keystore.setFaceId(answer: faceId)
        return keystore
    }()

    private static var faceId: SimFaceId {
        switch LaunchArgument.simFaceId.string {
        case "cancel": return .cancel
        case "nomatch": return .noMatch
        case "lockout": return .lockedOut
        case "notenrolled": return .notEnrolled
        default: return .match
        }
    }
}
#endif
