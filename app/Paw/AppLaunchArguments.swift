import SwiftUI

#if DEBUG
/// The app's own debug launch arguments (`KeyLaunchArgument` has the
/// simulated key's). All but the gallery's and the enclave check's are
/// read in the simulator only.
enum LaunchArgument: String, CaseIterable, DebugLaunchArgument {
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

    // The rest of the simulated world

    /// Word, `cancel|nomatch|lockout|notenrolled`: what the simulated Face ID answers.
    case simFaceId = "pawSimFaceId"
    /// Number: milliseconds the simulated clock moves on each time paw goes to the background.
    case simAwayMs = "pawSimAwayMs"
    /// Number: seconds the system keeps a copied password on the pasteboard.
    case simPasteboardSecs = "pawSimPasteboardSecs"
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
    /// Word, `changed|missing`: the fixture's `work/` names other keys (`FixtureSession`).
    case simRecipients = "pawSimRecipients"
    /// Word: how the simulated git host misbehaves at setup (`SetupLaunch`).
    case simRemote = "pawSimRemote"
    /// Word: what another computer does to the simulated git host (`SimRemoteChange`).
    case simRemoteChange = "pawSimRemoteChange"
    /// Flag: the simulated git host cuts the next connection off half way.
    case simRemoteDrop = "pawSimRemoteDrop"
    /// Word: the simulated GitHub, and how it behaves (`SimGitHubMode`).
    case simGitHub = "pawSimGitHub"

    static var colorScheme: ColorScheme? {
        switch appearance.string {
        case "dark": return .dark
        case "light": return .light
        default: return nil
        }
    }
}
#endif
