import SwiftUI

@main
struct PawApp: App {
    var body: some Scene {
        WindowGroup {
            #if DEBUG
            if GalleryLaunch.isRequested {
                GalleryRoot()
            } else {
                LaunchRoot()
            }
            #else
            LaunchRoot()
            #endif
        }
    }
}

/// There is no real store on a device yet, so outside the simulator's
/// fixture (`-pawFixture 1`) the app is still the key test screen.
private struct LaunchRoot: View {
    var body: some View {
        #if targetEnvironment(simulator)
        if FixtureLaunch.isRequested {
            FixtureRoot()
        } else {
            SelfTestView(host: "app")
        }
        #else
        SelfTestView(host: "app")
        #endif
    }
}
