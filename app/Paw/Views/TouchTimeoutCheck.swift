import SwiftUI

/// Key diagnostics' touch timeout check: the key is asked to sign once, and
/// nobody touches it, to learn what it answers when a touch times out. A
/// key task on the app's key port (`KeyPort.checkTouchTimeout`), so the PIN
/// goes only to the store's key, through the key session, as for an open.
/// What the key answered goes to the key log below it.
///
/// The check's section in Key diagnostics. `finished` runs when it ends,
/// so the key log can be read again.
struct TouchTimeoutSection: View {
    let model: AppModel
    let check: KeyTaskDriver
    let finished: () -> Void

    /// Started and not yet ended. Cancel ends it at once, even while the
    /// key still waits for a touch (`core/src/key/task.rs`).
    private var running: Bool {
        check.state.map { !$0.isFinal } ?? false
    }

    var body: some View {
        Section {
            if running {
                progress
                Button("Cancel", role: .cancel) { check.cancel() }
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("touch-timeout-cancel")
            } else {
                // No key, or another key: the task says so itself, before
                // any PIN.
                Button("Check the touch timeout") {
                    let port = model.keyPort
                    check.start { listener in port.checkTouchTimeout(listener: listener) }
                }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("touch-timeout-check")
                if let state = check.state {
                    result(state)
                }
            }
        } header: {
            PawSectionHeader("Touch timeout")
        }
        .onChange(of: running) { _, running in
            if !running { finished() }
        }
    }

    @ViewBuilder private var progress: some View {
        switch check.state {
        case .pin(let triesLeft, _, let lastError)?:
            if let lastError {
                let words = KeyErrorWords(pinAskedAgain: lastError, entry: "the check", purpose: .check)
                ErrorRow(kind: .error, problem: words.problem, nextStep: words.nextStep)
            } else if triesLeft < 3 {
                line(triesLeft == 1 ? "1 PIN try left before the key locks." : "\(triesLeft) PIN tries left.")
            }
            // As the key sheet's field: no content type, so iOS offers no
            // password AutoFill and no "Save Password?".
            SecureField("PIN", text: Bindable(check).pin)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .submitLabel(.continue)
                .onSubmit { check.continueWithPin() }
                .accessibilityIdentifier("pin-field")
            Button("Continue") { check.continueWithPin() }
                .foregroundStyle(PawColor.label)
                .disabled(check.pin.isEmpty)
                .accessibilityIdentifier("pin-continue")
        case .plugIn?:
            line("Plug in your key.")
        case .busy?:
            line("Your key is finishing something else.")
        case .touch?:
            line("Don't touch your key. It should give up after 15 seconds.")
                .accessibilityIdentifier("touch-timeout-waiting")
        default:
            line("Checking\u{2026}")
        }
    }

    @ViewBuilder private func result(_ state: KeyTaskState) -> some View {
        switch state {
        case .failed(.TouchTimeout):
            outcome("Your key gave up waiting for a touch.")
        case .done:
            outcome("Your key signed. It was touched, or it doesn't need a touch to sign in.")
        case .failed(.DifferentKey):
            outcome("This isn't the key this store was set up with. No PIN was sent.")
        case .failed(.Cancelled), .cancelled:
            EmptyView()
        case .failed(let error):
            let words = KeyErrorWords(error, entry: "the check", purpose: .check)
            ErrorRow(kind: .error, problem: words.problem, nextStep: words.nextStep)
                .accessibilityIdentifier("touch-timeout-result")
        default:
            EmptyView()
        }
    }

    private func line(_ text: String) -> some View {
        Text(text)
            .font(PawFont.subheadline)
            .foregroundStyle(PawColor.label)
            .fixedSize(horizontal: false, vertical: true)
    }

    private func outcome(_ text: String) -> some View {
        line(text)
            .accessibilityIdentifier("touch-timeout-result")
    }
}
