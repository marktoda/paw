import Observation
import SwiftUI

/// Key diagnostics' touch timeout check: the key is asked to sign once, and
/// nobody touches it, to learn what it answers when a touch times out. A
/// key task on the app's key port (`KeyPort.checkTouchTimeout`), so the PIN
/// goes only to the store's key, through the key session, as for an open.
/// What the key answered goes to the key log below it.
@MainActor @Observable
final class TouchTimeoutCheck {
    enum Outcome: Equatable {
        case timedOut
        case signed
        case differentKey
        case failed(KeyErrorWords)
    }

    /// Its stages, shown inline rather than in a sheet.
    let driver: KeyTaskDriver
    private(set) var outcome: Outcome?

    private static let words = KeyErrorWords.Context(entry: "the check")

    init(session: StoreSession) {
        driver = session.keyTasks.make()
    }

    /// Started and not yet ended. Cancel ends it at once, even while the
    /// key still waits for a touch (`core/src/key/task.rs`).
    var running: Bool {
        driver.isFollowing
    }

    /// Starts the check; `finished` runs when it ends or is cancelled, so
    /// the key log can be read again. No key, or another key: the task says
    /// so itself, before any PIN.
    func start(on port: KeyPort, finished: @escaping () -> Void) {
        driver.start(KeyJob<Never>(
            begin: { listener in port.checkTouchTimeout(listener: listener) },
            words: { Self.words },
            working: { _ in ("Checking\u{2026}", nil) },
            ended: { [weak self] end in
                self?.outcome = Self.outcome(end)
                return .close
            },
            starting: { [weak self] in
                self?.outcome = nil
            },
            closed: finished))
    }

    func cancel() {
        driver.cancel()
    }

    private static func outcome(_ end: KeyTaskEnd<Never>) -> Outcome? {
        switch end {
        case .done: .signed
        case .failed(.TouchTimeout): .timedOut
        case .failed(.DifferentKey): .differentKey
        case .failed(let error): .failed(words.words(error))
        case .cancelled: nil
        }
    }
}

struct TouchTimeoutSection: View {
    let model: AppModel
    let check: TouchTimeoutCheck
    let finished: () -> Void

    var body: some View {
        Section {
            if check.running {
                progress(check.driver.stage)
                Button("Cancel", role: .cancel) { check.cancel() }
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("touch-timeout-cancel")
            } else {
                Button("Check the touch timeout") {
                    check.start(on: model.storeSession.keyPort, finished: finished)
                }
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("touch-timeout-check")
                if let outcome = check.outcome {
                    result(outcome)
                }
            }
        } header: {
            PawSectionHeader("Touch timeout")
        }
    }

    /// The key sheet's stages, in its words, inline.
    @ViewBuilder private func progress(_ stage: KeySheetStage) -> some View {
        switch stage {
        case .pin, .wrongPin, .pinAgain:
            switch stage {
            case .wrongPin(let triesLeft):
                ErrorRow(
                    kind: .error, problem: stage.headline,
                    nextStep: KeySheetWords.triesLeft(triesLeft, afterWrongPin: false))
            case .pinAgain(_, let problem, let nextStep):
                ErrorRow(kind: .error, problem: problem, nextStep: nextStep)
            default:
                if let tries = stage.triesLine {
                    line(tries)
                }
            }
            KeyPinField(stage: stage, pin: Bindable(check.driver).pin) { check.driver.continueWithPin() }
            Button(stage.continueTitle) { check.driver.continueWithPin() }
                .foregroundStyle(PawColor.label)
                .disabled(check.driver.pin.count < 6)
                .accessibilityIdentifier("pin-continue")
        case .touch:
            line("Don't touch your key. It should give up after 15 seconds.")
                .accessibilityIdentifier("touch-timeout-waiting")
        default:
            line(stage.explanation ?? stage.headline)
        }
    }

    @ViewBuilder private func result(_ outcome: TouchTimeoutCheck.Outcome) -> some View {
        switch outcome {
        case .timedOut:
            self.outcome("Your key gave up waiting for a touch.")
        case .signed:
            self.outcome("Your key signed. It was touched, or it doesn't need a touch to sign in.")
        case .differentKey:
            self.outcome("This isn't the key this store was set up with. No PIN was sent.")
        case .failed(let words):
            ErrorRow(kind: .error, problem: words.problem, nextStep: words.nextStep)
                .accessibilityIdentifier("touch-timeout-result")
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
