import SwiftUI

/// Settings › AutoFill › Suggest one-time codes: whether paw keeps which
/// passwords have a one-time code, learned when one is opened or filled on
/// this iPhone, so a code field lists them and iOS suggests them (core
/// `OneTimeCodeEntries`, design 2.10). It is a fact learned from decrypted
/// text kept outside a sealed copy, so it is off until turned on (design
/// 5.6 rule 6). Off deletes the list and its suggestions; a code still
/// fills through search. The core keeps the switch beside the list, so the
/// AutoFill extension honours it too. Shown for a store paw can write, as
/// Sites used is.
struct OneTimeCodeSettingsSection: View {
    let model: AppModel
    @State private var enabled: Bool?
    @State private var problem: String?

    var body: some View {
        if let stateDir = model.settingsStateDir {
            let codes = OneTimeCodeEntries.open(stateDir: stateDir)
            Section {
                Toggle(isOn: binding(codes)) {
                    Text("Suggest one-time codes")
                        .foregroundStyle(PawColor.label)
                }
                .accessibilityIdentifier("settings-one-time-codes")
                if let problem {
                    ErrorRow(kind: .error, problem: "Code suggestions weren't changed", nextStep: problem)
                }
            } footer: {
                Text("Keeps a list of which passwords have a code.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
    }

    private func binding(_ codes: OneTimeCodeEntries) -> Binding<Bool> {
        Binding(
            get: { enabled ?? codes.enabled() },
            set: { set($0, codes) })
    }

    private func set(_ on: Bool, _ codes: OneTimeCodeEntries) {
        do {
            try codes.setEnabled(enabled: on)
            problem = nil
        } catch {
            problem = describe(error)
        }
        enabled = codes.enabled()
        // Off takes the code suggestions away.
        model.storeSession.autoFill?.republish()
    }
}
