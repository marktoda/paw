import SwiftUI

extension View {
    /// The bottom search field and round Add button, as the system draws
    /// them in glass. Use on the root view of a `NavigationStack`.
    func pawBottomSearch(
        text: Binding<String>, prompt: String, onAdd: @escaping () -> Void
    ) -> some View {
        searchable(text: text, prompt: Text(prompt))
            .toolbar {
                DefaultToolbarItem(kind: .search, placement: .bottomBar)
                ToolbarSpacer(.fixed, placement: .bottomBar)
                ToolbarItem(placement: .bottomBar) {
                    Button(action: onAdd) {
                        Label("Add a password", systemImage: PawSymbol.add)
                    }
                    .accessibilityIdentifier("add-button")
                }
            }
    }
}
