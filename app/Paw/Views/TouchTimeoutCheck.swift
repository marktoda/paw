import SwiftUI

/// Key diagnostics' touch timeout check: the key is asked to sign once, and
/// nobody touches it, to learn what it answers when a touch times out. A
/// key task on the app's key port (`KeyPort.checkTouchTimeout`), so the PIN
/// goes only to the store's key, through the key session, as for an open.
/// What the key answered goes to the key log below it.
@MainActor @Observable
final class TouchTimeoutCheck {
    private(set) var state: KeyTaskState?
    var pin = ""
    @ObservationIgnored private var task: KeyTask?

    var running: Bool {
        guard let state else { return false }
        return !state.isFinal
    }

    func start(on port: KeyPort) {
        task?.cancel()
        pin = ""
        let listener = CheckListener { [weak self] in
            MainActor.assumeIsolated { self?.refresh() }
        }
        task = port.checkTouchTimeout(listener: listener)
        // A task can start at the PIN stage; listeners hear only changes.
        refresh()
    }

    func sendPin() {
        guard let task, case .pin? = state, !pin.isEmpty else { return }
        task.supplyPin(pin: Data(pin.utf8))
        pin = ""
    }

    func cancel() {
        task?.cancel()
        refresh()
    }

    private func refresh() {
        guard let task else { return }
        state = task.state()
        if case .pin? = state {} else { pin = "" }
        if state?.isFinal == true {
            self.task = nil
        }
    }
}

private final class CheckListener: KeyTaskListener, @unchecked Sendable {
    private let changed: @Sendable () -> Void

    init(changed: @escaping @Sendable () -> Void) {
        self.changed = changed
    }

    func stateChanged(state: KeyTaskState) {
        DispatchQueue.main.async(execute: changed)
    }
}

/// The check's section in Key diagnostics. `finished` runs when it ends,
/// so the key log can be read again.
struct TouchTimeoutSection: View {
    let model: AppModel
    let check: TouchTimeoutCheck
    let finished: () -> Void

    var body: some View {
        Section {
            if check.running {
                progress
                Button("Cancel", role: .cancel) { check.cancel() }
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("touch-timeout-cancel")
            } else {
                // No key, or another key: the task says so itself, before
                // any PIN.
                Button("Check the touch timeout") { check.start(on: model.keyPort) }
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("touch-timeout-check")
                if let state = check.state {
                    result(state)
                }
            }
        } header: {
            PawSectionHeader("Touch timeout")
        } footer: {
            Text("Your key signs once. Don't touch it: the check is what it says when nobody does.")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
        }
        .onChange(of: check.running) { _, running in
            if !running { finished() }
        }
    }

    @ViewBuilder private var progress: some View {
        switch check.state {
        case .pin(let triesLeft, _, let lastError)?:
            if let lastError {
                let words = KeyErrorWords(pinAskedAgain: lastError, entry: "the check", purpose: .check)
                ErrorRow(kind: .error, problem: words.problem, nextStep: words.nextStep)
            } else if let triesLeft {
                line(triesLeft == 1 ? "1 PIN try left before the key locks." : "\(triesLeft) PIN tries left.")
            }
            // As the key sheet's field: no content type, so iOS offers no
            // password AutoFill and no "Save Password?".
            SecureField("PIN", text: Bindable(check).pin)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .submitLabel(.continue)
                .onSubmit { check.sendPin() }
                .accessibilityIdentifier("pin-field")
            Button("Continue") { check.sendPin() }
                .foregroundStyle(PawColor.label)
                .disabled(check.pin.isEmpty)
                .accessibilityIdentifier("pin-continue")
        case .plugIn?:
            line("Plug in your key.")
        case .busy?:
            line("Your key is finishing something else.")
        case .touch?:
            line("Don't touch your key. It gives up after about 15 seconds.")
                .accessibilityIdentifier("touch-timeout-waiting")
        default:
            line("Checking\u{2026}")
        }
    }

    @ViewBuilder private func result(_ state: KeyTaskState) -> some View {
        switch state {
        case .failed(.TouchTimeout):
            outcome("Your key gave up waiting for a touch. The key log below has what it answered.")
        case .done:
            outcome("Your key signed, so it was touched or it doesn't ask for a touch to sign in.")
        case .failed(.NotForThisKey):
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
