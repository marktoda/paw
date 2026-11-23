import SwiftUI

/// The navigation stack with Home at its root, the one key sheet, the
/// settings sheet and the app-switcher cover.
struct RootView: View {
    @Bindable var model: AppModel
    @Environment(\.scenePhase) private var scenePhase

    /// Swiping the key sheet down is Cancel: the core task stops too.
    private var keySheet: Binding<KeyRequest?> {
        Binding(
            get: { model.keyRequest },
            set: { request in
                if request == nil { model.cancelKeyTask() }
            })
    }

    var body: some View {
        NavigationStack(path: $model.path) {
            HomeScreen(model: model)
                .navigationDestination(for: Route.self) { route in
                    switch route {
                    case .folder(let path):
                        FolderScreen(model: model, path: path)
                    case .entry(let id):
                        if let session = model.session(id) {
                            EntryScreen(model: model, session: session)
                        } else {
                            ClosedEntryScreen()
                        }
                    case .anotherKey(let path):
                        AnotherKeyScreen(model: model, entry: EntryRef(path: path))
                    case .unpushed:
                        UnpushedScreen(model: model)
                    case .sync(let route):
                        SyncRouteScreen(model: model, route: route)
                    }
                }
        }
        .copyToast($model.toast)
        .pawEditing(model)
        .pawSync(model)
        .sheet(item: keySheet, onDismiss: { model.keySheetDismissed() }) { _ in
            KeySheetHost(model: model)
        }
        .sheet(isPresented: $model.showsSettings) {
            SettingsSheet(model: model)
        }
        .onChange(of: model.path) {
            model.pathChanged()
        }
        .onChange(of: scenePhase) {
            model.scenePhaseChanged(scenePhase)
        }
        .task {
            // Copies end with time, not only with taps: look again now and then.
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(30))
                model.refreshCopies()
            }
        }
        .privacyCover()
    }
}

/// What an entry route shows once its entry has been closed (Lock now).
private struct ClosedEntryScreen: View {
    var body: some View {
        EmptyState(
            symbol: PawSymbol.entry, title: "Closed",
            message: "This password was closed. Open it again from the list.")
    }
}

private struct PrivacyCover: ViewModifier {
    @Environment(\.scenePhase) private var scenePhase

    func body(content: Content) -> some View {
        content.overlay {
            // Names are sensitive too: the app switcher sees a blank window.
            if scenePhase != .active {
                PawColor.bg
                    .ignoresSafeArea()
                    .accessibilityElement()
                    .accessibilityLabel("paw is hidden")
                    .accessibilityIdentifier("privacy-cover")
            }
        }
    }
}

extension View {
    /// Covers the whole view whenever the scene is not active. Apply at the
    /// root and at the root of every sheet, which sit above the root.
    func privacyCover() -> some View {
        modifier(PrivacyCover())
    }
}
