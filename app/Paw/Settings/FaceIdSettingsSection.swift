import SwiftUI

/// Settings › Face ID: whether a password opened with the key stays
/// openable with Face ID for 8 hours (principles, decision of 2026-11-22).
/// Off ends every copy now, as Lock now does, and keeps none after: every
/// open needs the key. The core keeps the setting beside the copies, so the
/// AutoFill extension honours it too. Shown only where copies can be kept.
struct FaceIdSettingsSection: View {
    let model: AppModel
    @State private var enabled: Bool?
    @State private var problem: String?

    var body: some View {
        if let copies = model.storeSession.copies {
            Section {
                Toggle(isOn: binding(copies)) {
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
                Text(isOn(copies)
                     ? "For \(FaceIdWords.window) after you open a password with your key."
                     : "Every password needs your key.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("settings-face-id-footer")
            }
        }
    }

    private func isOn(_ copies: CopyCache) -> Bool {
        enabled ?? copies.copiesEnabled()
    }

    private func binding(_ copies: CopyCache) -> Binding<Bool> {
        Binding(
            get: { isOn(copies) },
            set: { set($0, copies) })
    }

    private func set(_ on: Bool, _ copies: CopyCache) {
        do {
            try copies.setCopiesEnabled(enabled: on)
            problem = nil
        } catch {
            problem = describe(error)
        }
        enabled = copies.copiesEnabled()
        // Home's "Open with Face ID" list, and any password open from a copy.
        model.refreshCopies()
    }
}
