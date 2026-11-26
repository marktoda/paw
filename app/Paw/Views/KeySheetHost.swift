import SwiftUI

/// The one key sheet, following `model.keyRequest`.
struct KeySheetHost: View {
    let model: AppModel

    var body: some View {
        KeyTaskSheet(
            driver: model.keyTask,
            purpose: "Open \(model.keyRequest?.entry.name ?? "")",
            note: model.storeSession.copies?.copiesEnabled() == true ? FaceIdWords.afterKey("opens") : nil,
            onCancel: { model.cancelKeyTask() })
            .keySheetPresentation(model.keyTask.stage)
    }
}

extension View {
    /// The key sheet as a sheet: 480pt, tall while the keyboard is up for
    /// the PIN; opaque, a form sheet on iPad, and covered in the app
    /// switcher. `stage` is `nil` while the sheet shows something else.
    func keySheetPresentation(_ stage: KeySheetStage?) -> some View {
        modifier(KeySheetPresentation(stage: stage))
    }
}

private struct KeySheetPresentation: ViewModifier {
    let stage: KeySheetStage?
    @State private var detent = PresentationDetent.height(PawMetrics.keySheetHeight)

    func body(content: Content) -> some View {
        content
            .presentationDetents([.height(PawMetrics.keySheetHeight), .large], selection: $detent)
            .presentationDragIndicator(.visible)
            .pawSheetBackground()
            .padFormSheet()
            .onChange(of: stage, initial: true) {
                detent = stage?.asksForPin == true ? .large : .height(PawMetrics.keySheetHeight)
            }
            .privacyCover()
    }
}
