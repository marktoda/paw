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
                        EntryRoute(model: model, id: id)
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
                Clipboard.clearIfExpired()
            }
        }
        .privacyCover()
        #if DEBUG && targetEnvironment(simulator)
        .background(alignment: .topLeading) {
            // For the UI tests: how many key tasks have not ended, so a
            // test can wait for "0" instead of sleeping before it checks
            // that a cancelled open left nothing.
            KeyTasksSignal(tasks: model.pendingKeyTasks)
        }
        #endif
    }
}

/// An entry route: the open entry, or why it closed. Its own view, so it
/// follows the model when the entry closes while on screen.
private struct EntryRoute: View {
    let model: AppModel
    let id: UUID

    var body: some View {
        if let session = model.session(id) {
            EntryScreen(model: model, session: session)
        } else {
            ClosedEntryScreen(model: model, id: id, closed: model.closedEntry(id))
        }
    }
}

/// What an entry route shows once its entry has been closed: why, and one
/// tap to open it again (with Face ID if its copy is still live, else with
/// the key, which the button's disc says).
private struct ClosedEntryScreen: View {
    let model: AppModel
    let id: UUID
    let closed: ClosedEntry?

    var body: some View {
        if let closed {
            VStack(spacing: PawMetrics.Spacing.l) {
                EmptyState(symbol: PawSymbol.entry, title: title(closed), message: message(closed))
                    .accessibilityIdentifier("entry-closed")
                Button {
                    model.reopen(id)
                } label: {
                    if model.liveCopy(closed.ref.path) != nil {
                        Text("Open with Face ID")
                    } else {
                        KeyButtonLabel(title: "Open with your key")
                    }
                }
                .buttonStyle(.pawPrimary)
                .padding(.horizontal, PawMetrics.Spacing.xl)
                .accessibilityIdentifier("entry-reopen")
            }
            .frame(maxHeight: .infinity)
            .background(PawColor.bg.ignoresSafeArea())
            .navigationTitle(closed.ref.name)
            .navigationBarTitleDisplayMode(.inline)
        } else {
            EmptyState(
                symbol: PawSymbol.entry, title: "Closed",
                message: "This password was closed. Open it again from the list.")
        }
    }

    private func title(_ closed: ClosedEntry) -> String {
        switch closed.reason {
        case .away: "Closed while you were away"
        case .deviceLocked: "Closed when your iPhone locked"
        case .copyEnded: "Its Face ID time ended"
        }
    }

    private func message(_ closed: ClosedEntry) -> String {
        switch closed.reason {
        case .away: "paw closes an open password after 2 minutes in the background."
        case .deviceLocked: "paw closes open passwords when your iPhone locks."
        case .copyEnded: "\(closed.ref.name) needs your key again."
        }
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
