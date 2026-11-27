import SwiftUI

#if DEBUG && targetEnvironment(simulator)
/// `-pawFixture 1`: the app on a made-up store, with the simulated key
/// (`-pawSimPin`, `-pawSimTouch`, `-pawSimTouchTimeoutMs`, `-pawSimPinTries`,
/// `-pawSimUnplugged`), a simulated
/// Face ID (`-pawSimFaceId ok|cancel|nomatch|lockout|notenrolled`) and a
/// simulated clock that the settings sheet can move on, and that moves on
/// by `-pawSimAwayMs` each time paw comes back from the background.
/// Simulator only.
/// `-pawAppearance dark|light` picks the theme. The store can be written
/// (`FixtureSession`, `-pawSimRecipients changed|missing`).
enum FixtureLaunch {
    static var isRequested: Bool {
        LaunchArgument.fixture.bool
    }

    /// Opens the fixture store, set up so it can be written; the plain
    /// files, read-only, if that fails (Home then says why).
    @MainActor
    static func makeModel() throws -> AppModel {
        do {
            let fixture = try FixtureSession(on: LaunchedHardware.app, writable: true, keyLog: true)
            return AppModel(session: fixture.session)
        } catch {
            let fixture = try FixtureSession(on: LaunchedHardware.app, writable: false, keyLog: true)
            let model = AppModel(session: fixture.session)
            model.notice = Notice(
                problem: "Adding passwords isn't available in this test store.", nextStep: describe(error))
            return model
        }
    }
}

/// Builds the fixture model once the first frame is up, and says so
/// inline if it cannot.
struct FixtureRoot: View {
    @State private var model: AppModel?
    @State private var problem: String?

    var body: some View {
        Group {
            if let model {
                RootView(model: model)
            } else if let problem {
                ErrorRow(kind: .error, problem: "The test store didn't open.", nextStep: problem)
                    .padding(PawMetrics.Spacing.l)
            } else {
                PawColor.bg.ignoresSafeArea()
            }
        }
        .preferredColorScheme(LaunchArgument.colorScheme)
        .onAppear {
            guard model == nil, problem == nil else { return }
            do {
                model = try FixtureLaunch.makeModel()
            } catch {
                problem = describe(error)
            }
        }
    }
}
#endif
