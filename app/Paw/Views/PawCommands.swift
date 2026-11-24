import SwiftUI

/// What the iPad's keyboard shortcuts run, published by the split view.
/// Each is what a button on screen does; `nil` turns the shortcut off.
/// Nothing here opens an entry or copies a secret, so a keyboard can't
/// get past the key or Face ID. ⌘C is on the open entry's own Copy
/// password button (`EntryScreen`), so it only exists once it is open.
struct PawActions {
    var search: (() -> Void)?
    var add: (() -> Void)?
    var check: (() -> Void)?
    var lock: (() -> Void)?
    var settings: (() -> Void)?
}

extension FocusedValues {
    @Entry var pawActions: PawActions?
}

/// The Passwords menu and Settings: listed when ⌘ is held, and in the
/// iPad's menu bar. Off everywhere but the iPad split view.
struct PawCommands: Commands {
    @FocusedValue(\.pawActions) private var actions

    var body: some Commands {
        CommandGroup(replacing: .appSettings) {
            item("Settings", ",", actions?.settings)
        }
        CommandMenu("Passwords") {
            item("Search", "f", actions?.search)
            item("New password", "n", actions?.add)
            item("Check for changes", "r", actions?.check)
            Divider()
            item("Lock now", "l", actions?.lock)
        }
    }

    @MainActor
    private func item(_ title: String, _ key: KeyEquivalent, _ action: (() -> Void)?) -> some View {
        Button(title) { action?() }
            .keyboardShortcut(key, modifiers: .command)
            .disabled(action == nil)
    }
}
