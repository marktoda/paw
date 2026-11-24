import SwiftUI
import UIKit

/// What differs on iPad. Every branch on the device goes through here, so
/// the iPhone keeps exactly the layout it had: on an iPhone each helper
/// below returns the view untouched.
enum PadLayout {
    @MainActor static var isPad: Bool {
        UIDevice.current.userInterfaceIdiom == .pad
    }

    /// The key sheet's column on iPad (design-language 4: "about 420pt").
    static let sheetColumn: CGFloat = 420
}

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

    /// A hardware-keyboard shortcut, on iPad only. It runs the button's own
    /// action, so it can do nothing the button can't.
    @MainActor @ViewBuilder
    func padShortcut(_ shortcut: KeyboardShortcut) -> some View {
        if PadLayout.isPad {
            keyboardShortcut(shortcut)
        } else {
            self
        }
    }

    /// At most `width` wide on iPad, centred by whatever frames it next.
    @MainActor @ViewBuilder
    func padMaxWidth(_ width: CGFloat) -> some View {
        if PadLayout.isPad {
            frame(maxWidth: width)
        } else {
            self
        }
    }

    /// Lists and scroll views keep to the readable width on iPad (560pt,
    /// design-language 3), with the ground filling the rest.
    @MainActor
    func padReadableWidth() -> some View {
        modifier(PadReadableWidth())
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

private struct PadReadableWidth: ViewModifier {
    @State private var width: CGFloat = 0

    @ViewBuilder
    func body(content: Content) -> some View {
        if PadLayout.isPad {
            content
                .contentMargins(.horizontal, max(0, (width - PawMetrics.readableWidth) / 2), for: .scrollContent)
                .onGeometryChange(for: CGFloat.self) { proxy in
                    proxy.size.width
                } action: { newWidth in
                    width = newWidth
                }
        } else {
            content
        }
    }
}
