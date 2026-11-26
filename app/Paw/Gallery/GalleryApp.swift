import SwiftUI

#if DEBUG && targetEnvironment(simulator)
/// The gallery's Home and Entry pages: the app's own screens on the
/// fixture store, brought to the state the page shows through the app's
/// model, as taps would. The key is a simulated one of the gallery's own
/// that needs no touch; the simulator's controls stay off the screens.
struct GalleryApp: View {
    let page: GalleryPage

    @State private var model: AppModel?
    @State private var problem: String?

    var body: some View {
        Group {
            if let model {
                RootView(model: model)
                    .environment(\.showsSimulatorControls, false)
            } else if let problem {
                ErrorRow(kind: .error, problem: "The gallery's store didn't open.", nextStep: problem)
                    .padding(PawMetrics.Spacing.l)
            } else {
                PawColor.bg.ignoresSafeArea()
            }
        }
        .task {
            guard model == nil, problem == nil else { return }
            do {
                model = try await Self.prepare(page)
            } catch {
                problem = describe(error)
            }
        }
    }

    /// Home: one change not sent, github.com open with Face ID, fastmail
    /// opened and locked again (Recent). Entry: first-example-bank open.
    @MainActor
    private static func prepare(_ page: GalleryPage) async throws -> AppModel {
        let key = SimulatedKey(pin: FixtureSession.pin, touchRequired: false, touchTimeoutMs: 15_000)
        let hardware = FixtureSession.OwnHardware(connect: { key.asConnection() })
        let model = AppModel(session: try FixtureSession(on: hardware.platform, writable: true).session)
        if page == .entry {
            try await FixtureSession.open("bank/first-example-bank", in: model)
        } else {
            try await FixtureSession.add("news.example.org", folder: "web", in: model)
            try await FixtureSession.open("email/fastmail", in: model)
            if case .entry(let id)? = model.path.last, let session = model.session(id) {
                model.lockEntry(session)
            }
            try await FixtureSession.open("web/github.com", in: model)
            model.setPath([])
        }
        model.toast = nil
        return model
    }
}
#endif
