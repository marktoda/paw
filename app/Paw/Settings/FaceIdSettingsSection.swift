import SwiftUI

/// Settings › Face ID: whether a password opened with the key stays
/// openable with Face ID for 8 hours (principles, decision of 2026-11-22).
/// Off ends every copy now, as Lock now does, and keeps none after: every
/// open needs the key. Where copies cannot be kept, the set-up store says
/// so here; a fixture says nothing.
struct FaceIdSettingsSection: View {
    let model: AppModel
    @State private var problem: String?

    var body: some View {
        if model.copies.cache != nil {
            Section {
                Toggle(isOn: Binding(get: { model.copies.enabled }, set: { set($0) })) {
                    Text(Device.words("Open with Face ID"))
                        .foregroundStyle(PawColor.label)
                }
                .accessibilityIdentifier("settings-face-id")
                if let problem {
                    ErrorRow(kind: .error, problem: "Face ID wasn't changed", nextStep: problem)
                }
            } header: {
                PawSectionHeader(Device.biometrics)
            } footer: {
                Text(model.copies.enabled
                     ? "For \(FaceIdWords.window) after you open a password with your key."
                     : "Every password needs your key.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("settings-face-id-footer")
            }
        } else if model.storeSession.setUp != nil {
            Section {
                Text(Device.words("Not available on this iPhone. Every password needs your key."))
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("settings-no-copies")
            } header: {
                PawSectionHeader(Device.biometrics)
            }
        }
    }

    private func set(_ on: Bool) {
        do {
            try model.copies.setEnabled(on)
            problem = nil
        } catch {
            problem = describe(error)
        }
    }
}
