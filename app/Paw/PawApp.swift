import SwiftUI
import UIKit

@main
struct PawApp: App {
    @UIApplicationDelegateAdaptor(PawAppDelegate.self) private var appDelegate

    var body: some Scene {
        WindowGroup {
            #if DEBUG
            if GalleryLaunch.isRequested {
                GalleryRoot()
            } else if EnclaveCheckLaunch.isRequested {
                EnclaveCheckView()
            } else {
                LaunchRoot()
            }
            #else
            LaunchRoot()
            #endif
        }
    }
}

/// Refuses third-party keyboards everywhere in paw: one would see every
/// PIN, password and note typed here. iOS already refuses them in secure
/// fields; this covers plain fields too (the editor's notes and names).
final class PawAppDelegate: NSObject, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        #if DEBUG && targetEnvironment(simulator)
        // UI tests: pushes, sheets and menus finish at once, so no step
        // waits for one to settle.
        if LaunchArguments.bool("pawNoAnimations") {
            UIView.setAnimationsEnabled(false)
        }
        #endif
        return true
    }

    func application(
        _ application: UIApplication,
        shouldAllowExtensionPointIdentifier extensionPointIdentifier: UIApplication.ExtensionPointIdentifier
    ) -> Bool {
        extensionPointIdentifier != .keyboard
    }
}

/// Setup until a store is set up, then Home on it (`StoreRoot`). In debug
/// simulator builds, `-pawFixture 1` opens the made-up store instead, and
/// `-pawSelfTest 1` the spike's key test screen.
private struct LaunchRoot: View {
    var body: some View {
        #if DEBUG && targetEnvironment(simulator)
        if AutoFillHarnessLaunch.isRequested {
            AutoFillHarnessRoot()
        } else if FixtureLaunch.isRequested {
            FixtureRoot()
        } else if LaunchArguments.bool("pawSelfTest") {
            SelfTestView(host: "app")
        } else {
            StoreRoot()
        }
        #else
        StoreRoot()
        #endif
    }
}
