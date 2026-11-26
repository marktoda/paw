import Foundation
import SwiftUI

#if DEBUG
/// Every debug launch argument paw reads (`-pawFixture 1`, `-pawSimPin
/// 123456`), each read from this launch's command line only. `UserDefaults`
/// would also answer from values persisted in the app's defaults, which
/// survive launches and could switch a hook on with nobody passing the
/// argument.
///
/// Debug builds only: a Release build reads no launch argument at all, so
/// nothing passed on its command line can switch a harness on. All but the
/// gallery's and the enclave check's are read in the simulator only.
///
/// `scripts/launch-arguments.sh` reads this list: every argument a UI test
/// passes must be a case here, every case must be read somewhere in the
/// app, and `scripts/check-no-sim.sh` looks for every name in the device
/// build.
enum LaunchArgument: String, CaseIterable {
    // What the app opens

    /// Flag: the made-up fixture store instead of the set-up one (`FixtureLaunch`).
    case fixture = "pawFixture"
    /// Flag: setup from nothing, against the simulated git host (`SetupLaunch`).
    case setup = "pawSetup"
    /// Flag: with `setup`, keep what earlier launches set up.
    case setupKeep = "pawSetupKeep"
    /// Word, `dark` or `light`: the theme of the fixture, the gallery and the AutoFill harness.
    case appearance = "pawAppearance"
    /// Flag: UIKit's animations off.
    case noAnimations = "pawNoAnimations"
    /// Flag: the component gallery instead of the app (`GalleryLaunch`).
    case gallery = "pawGallery"
    /// Word: the gallery page to open (`GalleryPage`).
    case galleryPage = "pawGalleryPage"
    /// Flag: the Secure Enclave and clock check screen (`EnclaveCheckView`).
    case enclaveCheck = "pawEnclaveCheck"

    // The AutoFill harness (`AutoFillHarnessLaunch`)

    /// Words, comma-separated: the sites AutoFill asks for, one per run.
    case hostProvider = "pawHostProvider"
    /// Word: the entry of a tapped suggestion.
    case hostProviderEntry = "pawHostProviderEntry"
    /// Word, `none` or `busy`: no store set up, or busy the first time.
    case hostProviderStore = "pawHostProviderStore"
    /// Flag: the harness's view goes away without paw ending the request.
    case harnessDismiss = "pawHarnessDismiss"

    // The simulated key (`SimulatedKeyHolder`)

    /// Word: its PIN (default 123456).
    case simPin = "pawSimPin"
    /// Flag: it needs a touch (default on).
    case simTouch = "pawSimTouch"
    /// Number: how long it waits for the touch, in milliseconds.
    case simTouchTimeoutMs = "pawSimTouchTimeoutMs"
    /// Number: PIN tries left.
    case simPinTries = "pawSimPinTries"
    /// Word, `unsupported`: a PIN KDF paw can't follow.
    case simKdf = "pawSimKdf"
    /// Number: how many of its first reads fail.
    case simAttachFails = "pawSimAttachFails"
    /// Flag: it is out at launch.
    case simUnplugged = "pawSimUnplugged"

    // The rest of the simulated world

    /// Word, `cancel|nomatch|lockout|notenrolled`: what the simulated Face ID answers.
    case simFaceId = "pawSimFaceId"
    /// Number: milliseconds the simulated clock moves on each time paw goes to the background.
    case simAwayMs = "pawSimAwayMs"
    /// Number: seconds the system keeps a copied password on the pasteboard.
    case simPasteboardSecs = "pawSimPasteboardSecs"
    /// Flag: the privacy cover stays up.
    case simPrivacyCover = "pawSimPrivacyCover"
    /// Flag: launch as if the phone were locked (`SimLockedLaunch`).
    case simLockedLaunch = "pawSimLockedLaunch"
    /// Flag: a store left where paw kept it before (`LegacyStore`).
    case plantLegacy = "pawPlantLegacy"
    /// Flag: Sites used starts with a few pairings.
    case simPairings = "pawSimPairings"
    /// Flag: "Suggest one-time codes" on in the AutoFill harness.
    case simOneTimeCodes = "pawSimOneTimeCodes"
    /// Flag: the background check runs on every trip to the background.
    case simBackgroundRefresh = "pawSimBackgroundRefresh"
    /// Word, `changed|missing`: the fixture's `work/` names other keys (`WritableFixture`).
    case simRecipients = "pawSimRecipients"
    /// Word: how the simulated git host misbehaves at setup (`SetupLaunch`).
    case simRemote = "pawSimRemote"
    /// Word: what another computer does to the simulated git host (`SimRemoteChange`).
    case simRemoteChange = "pawSimRemoteChange"
    /// Flag: the simulated git host refuses pushes.
    case simRemoteRefusePushes = "pawSimRemoteRefusePushes"
    /// Flag: the simulated git host cuts the next connection off half way.
    case simRemoteDrop = "pawSimRemoteDrop"
    /// Word: the simulated GitHub, and how it behaves (`SimGitHubMode`).
    case simGitHub = "pawSimGitHub"

    /// The word after `-name`, if this launch was given it.
    var string: String? {
        let arguments = ProcessInfo.processInfo.arguments
        guard let index = arguments.firstIndex(of: "-" + rawValue), arguments.indices.contains(index + 1) else {
            return nil
        }
        return arguments[index + 1]
    }

    /// `-name 1` (or YES, true).
    var bool: Bool {
        guard let value = string?.lowercased() else { return false }
        return ["1", "yes", "true"].contains(value)
    }

    var int: Int? {
        string.flatMap { Int($0) }
    }

    /// `-pawAppearance dark|light`.
    static var colorScheme: ColorScheme? {
        switch appearance.string {
        case "dark": return .dark
        case "light": return .light
        default: return nil
        }
    }
}
#endif
