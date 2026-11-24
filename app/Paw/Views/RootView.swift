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
            // And how many times the privacy cover has been drawn.
            Text(verbatim: "\(PrivacyCoverLog.shared.shown)")
                .font(.caption2)
                .frame(width: 1, height: 1)
                .opacity(0.02)
                .accessibilityLabel("Privacy cover drawn")
                .accessibilityValue("\(PrivacyCoverLog.shared.shown)")
                .accessibilityIdentifier("sim-privacy-cover-shown")
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
                EmptyState(symbol: PawSymbol.entry, title: title(closed), message: "")
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel(title(closed))
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
                message: "Open it again from the list.")
        }
    }

    private func title(_ closed: ClosedEntry) -> String {
        switch closed.reason {
        case .away: "Closed after 2 minutes away"
        case .deviceLocked: "Closed when your iPhone locked"
        case .copyEnded: "Its Face ID time ended"
        }
    }
}

private struct PrivacyCover: ViewModifier {
    @Environment(\.scenePhase) private var scenePhase

    #if DEBUG && targetEnvironment(simulator)
    /// `-pawSimPrivacyCover 1`: the cover stays up, so a test can see it.
    private static let held = LaunchArguments.bool("pawSimPrivacyCover")
    #else
    private static let held = false
    #endif

    func body(content: Content) -> some View {
        content.overlay {
            // Names are sensitive too: the app switcher sees only the name
            // on an opaque background.
            if scenePhase != .active || Self.held {
                ZStack {
                    PawColor.bg
                    Text(verbatim: "paw")
                        .font(PawFont.homeTitle)
                        .foregroundStyle(PawColor.label)
                }
                .ignoresSafeArea()
                .accessibilityElement()
                .accessibilityLabel("paw is hidden")
                .accessibilityIdentifier("privacy-cover")
                #if DEBUG && targetEnvironment(simulator)
                .onAppear { PrivacyCoverLog.shared.shown += 1 }
                #endif
            }
        }
    }
}

#if DEBUG && targetEnvironment(simulator)
/// For the UI tests: the cover is up only while paw is not active, when a
/// test can't look at it, so it counts each time it is drawn.
@MainActor @Observable
final class PrivacyCoverLog {
    static let shared = PrivacyCoverLog()
    var shown = 0
}
#endif

extension View {
    /// Covers the whole view whenever the scene is not active. Apply at the
    /// root and at the root of every sheet, which sit above the root.
    func privacyCover() -> some View {
        modifier(PrivacyCover())
    }
}
