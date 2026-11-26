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

    private struct Stuck: LocalizedError {
        let errorDescription: String?
    }

    /// Home: one change not sent, github.com open with Face ID, fastmail
    /// opened and locked again (Recent). Entry: first-example-bank open.
    @MainActor
    private static func prepare(_ page: GalleryPage) async throws -> AppModel {
        let key = SimulatedKey(pin: FixtureSession.pin, touchRequired: false, touchTimeoutMs: 15_000)
        let fixture = try FixtureSession(
            key: .own(key, connect: { key.asConnection() }), writable: true, simulatorControls: false)
        let model = AppModel(session: fixture.session)
        if page == .entry {
            try await open("bank/first-example-bank", in: model)
        } else {
            try await add("news.example.org", folder: "web", in: model)
            try await open("email/fastmail", in: model)
            if case .entry(let id)? = model.path.last, let session = model.session(id) {
                model.lockEntry(session)
            }
            try await open("web/github.com", in: model)
            model.setPath([])
        }
        model.toast = nil
        return model
    }

    /// A tap on the entry's row, the PIN typed if the key asks for it, and
    /// the sheet gone at Done: the entry is on top of the stack.
    @MainActor
    private static func open(_ path: String, in model: AppModel) async throws {
        model.open(EntryRef(path: path))
        var typed = false
        try await waitUntil("\(path) to open") {
            switch model.keyTask.stage {
            case .pin where !typed:
                typed = true
                model.keyTask.pin = FixtureSession.pin
                model.keyTask.continueWithPin()
            case .done:
                return true
            default:
                break
            }
            return false
        }
        model.keyRequest = nil
        model.keySheetDismissed()
    }

    /// A new entry saved from the add form, as its Save does.
    @MainActor
    private static func add(_ name: String, folder: String, in model: AppModel) async throws {
        guard let edits = model.storeSession.edits else {
            throw Stuck(errorDescription: "The fixture store can't be written.")
        }
        let live = model.storeSession.live
        let form = EntryFormModel(adding: folder, writer: edits.writer, folders: live.store.allFolders(), live: live)
        form.name = name
        form.password = "Typed-pass-42"
        form.changed()
        try await waitUntil("who can open \(folder)") { form.folderWritable != nil }
        guard form.canSave, model.save(form) else {
            throw Stuck(errorDescription: "\(name) wasn't saved.")
        }
    }

    @MainActor
    private static func waitUntil(_ what: String, _ condition: () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(30)
        while !condition() {
            if Date() > deadline {
                throw Stuck(errorDescription: "Waited 30 s for \(what).")
            }
            try await Task.sleep(for: .milliseconds(10))
        }
    }
}
#endif
