import AuthenticationServices
import SwiftUI

/// Settings › AutoFill: whether paw is turned on as a password provider,
/// and the way to turn it on. App target only.
struct AutoFillSettingsSection: View {
    /// The set-up store's suggestions; `nil` for a fixture.
    let suggestions: AutoFillSuggestions?
    @Environment(\.scenePhase) private var scenePhase
    @State private var enabled: Bool?
    /// iOS's own prompt was refused (or not shown): the button then opens
    /// paw's page in Settings instead of asking again.
    @State private var declined = false

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
            // "Turn on" until AutoFill is known to be on, so the row doesn't
            // change when the state arrives a moment after Settings opens.
            if enabled != true && !declined {
                Button("Turn on") { turnOn() }
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("settings-autofill-turn-on")
            } else {
                Button("Open Settings") { openSettings() }
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("settings-autofill-open")
            }
        } footer: {
            Text("In Safari and apps.")
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
        if now, was == false {
            suggestions?.republish()
        }
    }

    /// iOS asks the person in its own prompt. On yes, the suggestions are
    /// written at once.
    private func turnOn() {
        Task {
            let accepted = await ASSettingsHelper.requestToTurnOnCredentialProviderExtension()
            if accepted {
                await readState()
            } else {
                declined = true
            }
        }
    }

    private func openSettings() {
        Task {
            try? await ASSettingsHelper.openCredentialProviderAppSettings()
        }
    }
}
