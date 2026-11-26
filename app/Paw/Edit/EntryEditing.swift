import SwiftUI

/// The open entry's Edit button and its menu: Rename or move, Delete.
private struct EntryEditingModifier: ViewModifier {
    let model: AppModel
    let session: OpenSession

    func body(content: Content) -> some View {
        content.toolbar {
            if let edits = model.storeSession.edits {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Edit") { edits.startEdit(session) }
                        .accessibilityIdentifier("entry-edit")
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button {
                            edits.startEdit(session, focusName: true)
                        } label: {
                            Label("Rename or move", systemImage: "folder")
                        }
                        .accessibilityIdentifier("entry-rename")
                        Button(role: .destructive) {
                            edits.confirmDelete(session)
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                        .accessibilityIdentifier("entry-delete")
                    } label: {
                        Label("More", systemImage: "ellipsis")
                    }
                    .accessibilityIdentifier("entry-more")
                }
            }
        }
    }
}

/// "Delete github.com? It stays in the store's history." at the top of
/// the open entry, after Delete in its menu.
struct EntryDeleteConfirm: View {
    let model: AppModel
    let session: OpenSession

    var body: some View {
        if let edits = model.storeSession.edits, edits.confirmingDelete == session.id {
            Section {
                InlineConfirmRow(
                    question: "Delete \(session.ref.name)?",
                    detail: "It stays in the store's history.",
                    confirmTitle: "Delete", cancelTitle: "Keep it", identifier: "entry-delete",
                    onConfirm: { edits.delete(session) },
                    onCancel: { edits.keepEntry() })
                    .pawListRow()
            }
        }
    }
}

extension View {
    func entryEditing(_ model: AppModel, session: OpenSession) -> some View {
        modifier(EntryEditingModifier(model: model, session: session))
    }
}
