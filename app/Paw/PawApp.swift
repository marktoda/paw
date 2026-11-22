import SwiftUI

@main
struct PawApp: App {
    var body: some Scene {
        WindowGroup {
            #if DEBUG
            if GalleryLaunch.isRequested {
                GalleryRoot()
            } else {
                SelfTestView(host: "app")
            }
            #else
            SelfTestView(host: "app")
            #endif
        }
    }
}
