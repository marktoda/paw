import AuthenticationServices
import SwiftUI

/// Settings › AutoFill: whether paw is turned on as a password provider,
/// and the way to turn it on. App target only.
struct AutoFillSettingsSection: View {
    @Environment(\.scenePhase) private var scenePhase
    @State private var enabled: Bool?

    var body: some View {
        Section {
            LabeledContent {
                Text(stateWords)
                    .foregroundStyle(PawColor.labelSecondary)
            } label: {
                Text("AutoFill")
                    .foregroundStyle(PawColor.label)
            }
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("settings-autofill")
            // Shown until AutoFill is known to be on, so the rows below
            // don't move when the state arrives a moment after Settings opens.
            if enabled != true {
                Button("Turn on AutoFill") { turnOn() }
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("settings-autofill-turn-on")
            }
            Button("Open AutoFill settings") { openSettings() }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("settings-autofill-open")
        } header: {
            PawSectionHeader("AutoFill")
        } footer: {
            Text("Fills passwords in Safari and apps. Turn it on in Settings \u{203A} General \u{203A} AutoFill & Passwords.")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
        }
        .task(id: scenePhase) {
            await readState()
        }
    }

    private var stateWords: String {
        switch enabled {
        case true?: "On"
        case false?: "Off"
        case nil: ""
        }
    }

    private func readState() async {
        let was = enabled
        let now = await ASCredentialIdentityStore.shared.state().isEnabled
        enabled = now
        // Turned on in Settings since: write the suggestions now.
        if now, was == false, let store = CredentialIdentities.current {
            CredentialIdentities.publish(store)
        }
    }

    /// iOS asks the person in its own prompt. On yes, the suggestions are
    /// written at once.
    private func turnOn() {
        Task {
            let accepted = await ASSettingsHelper.requestToTurnOnCredentialProviderExtension()
            if accepted {
                await readState()
            }
        }
    }

    private func openSettings() {
        Task {
            try? await ASSettingsHelper.openCredentialProviderAppSettings()
        }
    }
}
