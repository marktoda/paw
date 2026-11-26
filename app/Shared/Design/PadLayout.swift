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
    /// A hardware-keyboard shortcut, on iPad only, and only while `enabled`
    /// (off while a sheet covers the button). It runs the button's own
    /// action, so it can do nothing the button can't.
    @MainActor @ViewBuilder
    func padShortcut(_ shortcut: KeyboardShortcut, enabled: Bool = true) -> some View {
        if PadLayout.isPad {
            let active: KeyboardShortcut? = enabled ? shortcut : nil
            keyboardShortcut(active)
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
}

private struct PadReadableWidth: ViewModifier {
    @State private var width: CGFloat = 0

    @ViewBuilder
    func body(content: Content) -> some View {
        if PadLayout.isPad {
            content
                // Never less than the screen margin: in a narrow column (the
                // split view's right) the rows keep clear of the divider and
                // the edge instead of running into them.
                .contentMargins(
                    .horizontal, max(PawMetrics.screenMargin, (width - PawMetrics.readableWidth) / 2),
                    for: .scrollContent)
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
