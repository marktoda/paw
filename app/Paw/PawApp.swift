import SwiftUI

@main
struct PawApp: App {
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

/// Setup until a store is set up, then Home on it (`StoreRoot`). In the
/// simulator, `-pawFixture 1` opens the made-up store instead, and
/// `-pawSelfTest 1` the key test screen.
private struct LaunchRoot: View {
    var body: some View {
        #if targetEnvironment(simulator)
        if FixtureLaunch.isRequested {
            FixtureRoot()
        } else if UserDefaults.standard.bool(forKey: "pawSelfTest") {
            SelfTestView(host: "app")
        } else {
            StoreRoot()
        }
        #else
        StoreRoot()
        #endif
    }
}
