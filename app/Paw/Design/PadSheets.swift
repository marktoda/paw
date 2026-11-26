import Observation
import SwiftUI

/// How the app's own sheets present on iPad.
extension View {
    /// On iPad: a centred form sheet with an opaque ground. Apply to a
    /// sheet's content, next to its detents (which iPad ignores in regular
    /// width and keeps in Slide Over).
    @MainActor @ViewBuilder
    func padFormSheet() -> some View {
        if PadLayout.isPad {
            presentationSizing(.form)
                .presentationBackground(PawColor.bg)
        } else {
            self
        }
    }

    /// A sheet on iPhone; on iPad the whole screen (large type). Counted
    /// in `SheetsUp`, as `pawSheet` is.
    @MainActor @ViewBuilder
    func sheetOrPadCover<Item: Identifiable, Sheet: View>(
        item: Binding<Item?>, @ViewBuilder content: @escaping (Item) -> Sheet
    ) -> some View {
        if PadLayout.isPad {
            fullScreenCover(item: item, content: content)
                .modifier(CountedSheet(isUp: item.wrappedValue != nil))
        } else {
            pawSheet(item: item, content: content)
        }
    }

    /// How every app sheet but a key sheet is presented: counted in
    /// `SheetsUp` while it is up.
    @MainActor
    func pawSheet<Item: Identifiable, Sheet: View>(
        item: Binding<Item?>, onDismiss: (() -> Void)? = nil, @ViewBuilder content: @escaping (Item) -> Sheet
    ) -> some View {
        sheet(item: item, onDismiss: onDismiss, content: content)
            .modifier(CountedSheet(isUp: item.wrappedValue != nil))
    }

    @MainActor
    func pawSheet<Sheet: View>(
        isPresented: Binding<Bool>, onDismiss: (() -> Void)? = nil, @ViewBuilder content: @escaping () -> Sheet
    ) -> some View {
        sheet(isPresented: isPresented, onDismiss: onDismiss, content: content)
            .modifier(CountedSheet(isUp: isPresented.wrappedValue))
    }
}

/// The app's sheets that are up, but for key sheets, whose drivers say
/// theirs (`KeyTaskDrivers.sheetShown`): each is presented with `pawSheet`
/// or `sheetOrPadCover`, which count it here. The iPad's keyboard
/// shortcuts do nothing while one is up (`AppModel.sheetIsUp`).
@MainActor @Observable
final class SheetsUp {
    private var up: Set<UUID> = []

    var any: Bool {
        !up.isEmpty
    }

    fileprivate func mark(_ id: UUID, up isUp: Bool) {
        if isUp {
            up.insert(id)
        } else {
            up.remove(id)
        }
    }
}

extension EnvironmentValues {
    /// Where the sheets presented below count themselves; `nil` outside
    /// an open store (setup has no shortcuts).
    @Entry var sheetsUp: SheetsUp? = nil
}

private struct CountedSheet: ViewModifier {
    let isUp: Bool
    @Environment(\.sheetsUp) private var sheetsUp
    @State private var id = UUID()

    func body(content: Content) -> some View {
        content
            .onChange(of: isUp, initial: true) {
                sheetsUp?.mark(id, up: isUp)
            }
            .onDisappear {
                sheetsUp?.mark(id, up: false)
            }
    }
}

extension View {
    /// An opaque ground for a sheet that opens part-way up. The system's
    /// glass sheet lets the screen behind show through at a small detent,
    /// and its rows end up under the sheet's own words.
    func pawSheetBackground() -> some View {
        presentationBackground(PawColor.bg)
    }
}
