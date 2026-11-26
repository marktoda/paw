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

    /// A sheet on iPhone; on iPad the whole screen (large type).
    @MainActor @ViewBuilder
    func sheetOrPadCover<Item: Identifiable, Sheet: View>(
        item: Binding<Item?>, @ViewBuilder content: @escaping (Item) -> Sheet
    ) -> some View {
        if PadLayout.isPad {
            fullScreenCover(item: item, content: content)
        } else {
            sheet(item: item, content: content)
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
