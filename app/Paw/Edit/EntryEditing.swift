import SwiftUI

/// The open entry's Edit button and its menu: Rename or move, Delete.
private struct EntryEditingModifier: ViewModifier {
    let model: AppModel
    let session: OpenSession

    func body(content: Content) -> some View {
        content.toolbar {
            if model.edits != nil {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Edit") { model.startEdit(session) }
                        .accessibilityIdentifier("entry-edit")
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button {
                            model.startEdit(session, focusName: true)
                        } label: {
                            Label("Rename or move", systemImage: "folder")
                        }
                        .accessibilityIdentifier("entry-rename")
                        Button(role: .destructive) {
                            model.edits?.confirmingDelete = session.id
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
        if model.edits?.confirmingDelete == session.id {
            Section {
                InlineConfirmRow(
                    question: "Delete \(session.ref.name)?",
                    detail: "It stays in the store's history.",
                    confirmTitle: "Delete", cancelTitle: "Keep it", identifier: "entry-delete",
                    onConfirm: { model.delete(session) },
                    onCancel: { model.edits?.confirmingDelete = nil })
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
