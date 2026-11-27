import SwiftUI

/// Where the one key sheet is. Plain values only: the core decides the stage.
enum KeySheetStage: Equatable {
    case plugIn
    /// `triesLeft` is shown before any input when few remain
    /// (`KeySheetWords.warnsBelowTries`). `forgotten` adds the line about the PIN being dropped in the background.
    case pin(triesLeft: Int, forgotten: Bool)
    case wrongPin(triesLeft: Int)
    /// The PIN is asked again for another reason: it was not 6 to 127
    /// characters (no try used), or the key no longer takes the PIN paw had.
    case pinAgain(triesLeft: Int, problem: String, nextStep: String)
    /// The key is finishing something else.
    case busy
    /// The key is blinking and waiting for a touch.
    case touch
    case working(message: String, progress: String?)
    /// Touch received. The host closes the sheet after a second.
    case done
    /// Out of PIN tries.
    case locked
    /// Something went wrong that trying again may fix, e.g. the key was not
    /// touched in time. `problem` says what happened, `nextStep` what to do.
    case failed(problem: String, nextStep: String)

    var asksForPin: Bool {
        switch self {
        case .pin, .wrongPin, .pinAgain: true
        default: false
        }
    }
}

/// The content of the key sheet for every stage. The host presents it with
/// `.presentationDetents([.height(PawMetrics.keySheetHeight), .large])`.
struct KeySheetContent: View {
    /// What the key is for, e.g. "Open Fastmail".
    let purpose: String
    let stage: KeySheetStage
    @Binding var pin: String
    /// A line under the steps at Touch (`FaceIdWords.afterKey`).
    var note: String? = nil
    var onContinue: () -> Void = {}
    var onCancel: () -> Void = {}
    /// "Try again" on the failed stage.
    var onRetry: () -> Void = {}

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    #if DEBUG && targetEnvironment(simulator)
    @Environment(\.simulatorActions) private var simulatorActions
    #endif
    @State private var showTouchHint = false

    var body: some View {
        GeometryReader { proxy in
            ScrollView {
                content
                    .padMaxWidth(PadLayout.sheetColumn - 2 * PawMetrics.screenMargin)
                    .padding(.horizontal, PawMetrics.screenMargin)
                    .padding(.top, PawMetrics.Spacing.xl)
                    .padding(.bottom, PawMetrics.Spacing.s)
                    // Without a keyboard the buttons sit at the bottom of the sheet.
                    .frame(
                        maxWidth: .infinity,
                        minHeight: stage.asksForPin ? nil : proxy.size.height,
                        alignment: .top)
            }
            .scrollBounceBehavior(.basedOnSize)
        }
        .onChange(of: stage, initial: true) {
            stageChanged()
        }
        .sensoryFeedback(trigger: stage) { _, new in
            Self.haptic(for: new)
        }
    }

    private var content: some View {
        VStack(spacing: PawMetrics.Spacing.l) {
            Group {
                Text(Device.words(purpose))
                    .font(PawFont.sectionHeader)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("key-sheet-purpose")
                hero
                words
                if stage.asksForPin {
                    KeyPinField(stage: stage, pin: $pin, onContinue: onContinue)
                }
                if showsSteps {
                    KeyStepPills(current: currentStep)
                }
                if let note, stage == .touch {
                    Text(Device.words(note))
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: PawMetrics.sheetTextWidth)
                        .accessibilityIdentifier("key-sheet-note")
                }
            }
            if !stage.asksForPin {
                Spacer(minLength: 0)
            }
            buttons
            #if DEBUG && targetEnvironment(simulator)
            SimulatorStrip(actions: simulatorActions)
            #endif
        }
    }

    @ViewBuilder private var hero: some View {
        switch stage {
        case .plugIn, .busy:
            KeyDisc(size: .hero)
        case .pin, .wrongPin, .pinAgain:
            // Smaller, so the field and both buttons stay above the keyboard.
            KeyDisc(size: .banner)
                .accessibilityHidden(true)
        case .touch:
            KeyDrawing()
                .onTapGesture { showTouchHint = true }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("Your key, waiting for a touch")
                .accessibilityIdentifier("touch-prompt")
        case .working:
            ProgressView()
                .controlSize(.large)
                .frame(height: PawMetrics.discHero)
        case .done:
            Image(systemName: PawSymbol.synced)
                .font(.system(size: PawMetrics.discHero))
                .foregroundStyle(PawColor.success)
                .accessibilityLabel("Done")
                .transition(reduceMotion ? .opacity : .scale(scale: 0.6).combined(with: .opacity))
        case .locked:
            KeyDisc(size: .hero, phase: .locked)
        case .failed:
            // Trying again needs the key, so the disc stays; smaller, so
            // both buttons fit the sheet.
            KeyDisc(size: .banner)
                .accessibilityHidden(true)
        }
    }

    private var words: some View {
        VStack(spacing: 6) {
            Text(Device.words(stage.headline))
                .font(PawFont.title)
                .foregroundStyle(PawColor.label)
                .multilineTextAlignment(.center)
                .accessibilityAddTraits(.isHeader)
                .accessibilityIdentifier("key-sheet-headline")
            if let triesLine = stage.triesLine {
                Label(triesLine, systemImage: PawSymbol.warning)
                    .font(PawFont.numeral)
                    .foregroundStyle(PawColor.warning)
                    .multilineTextAlignment(.center)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(triesLine)
                    .accessibilityIdentifier("pin-tries-left")
            }
            if let explanation = stage.explanation {
                Text(Device.words(explanation))
                    .font(PawFont.subheadline)
                    .foregroundStyle(PawColor.labelSecondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: PawMetrics.sheetTextWidth)
                    .accessibilityIdentifier("key-sheet-body")
            }
            if showTouchHint, stage == .touch {
                Text("Touch the key itself, not the screen.")
                    .font(PawFont.subheadline.weight(.semibold))
                    .foregroundStyle(PawColor.label)
                    .multilineTextAlignment(.center)
                    .accessibilityIdentifier("touch-hint")
            }
        }
    }

    @ViewBuilder private var buttons: some View {
        VStack(spacing: PawMetrics.Spacing.s) {
            switch stage {
            case .pin, .wrongPin, .pinAgain:
                Button(stage.continueTitle, action: onContinue)
                    .buttonStyle(.pawPrimary)
                    .disabled(!canContinue)
                    .accessibilityIdentifier("pin-continue")
                cancelButton("Cancel")
            case .locked:
                cancelButton("Close")
            case .failed:
                Button("Try again", action: onRetry)
                    .buttonStyle(.pawPrimary)
                    .accessibilityIdentifier("key-sheet-retry")
                cancelButton("Cancel")
            case .done:
                EmptyView()
            case .plugIn, .busy, .touch, .working:
                cancelButton("Cancel")
            }
        }
    }

    private func cancelButton(_ title: String) -> some View {
        Button(title, action: onCancel)
            .buttonStyle(.pawSecondary)
            .padShortcut(.cancelAction)
            .accessibilityIdentifier("key-sheet-cancel")
    }

    private static func haptic(for stage: KeySheetStage) -> SensoryFeedback? {
        switch stage {
        case .wrongPin(let triesLeft): return triesLeft == 1 ? .warning : .error
        case .pinAgain: return .error
        case .done: return .success
        default: return nil
        }
    }

    private var announcement: String {
        Device.words(spokenSentences(stage.headline, stage.triesLine, stage.explanation))
    }

    private var showsSteps: Bool {
        switch stage {
        case .locked, .done, .failed: false
        default: true
        }
    }

    private var canContinue: Bool {
        pin.utf8.count >= Int(pinRules().minLength)
    }

    private var currentStep: Int {
        switch stage {
        case .plugIn: 0
        case .pin, .wrongPin, .pinAgain, .locked: 1
        case .busy, .touch, .failed: 2
        case .working, .done: 3
        }
    }

    private func stageChanged() {
        showTouchHint = false
        AccessibilityNotification.Announcement(announcement).post()
    }
}

/// The PIN stage's field, in the key sheet and inline (Key diagnostics'
/// touch check). One secure field whatever Show says: a plain text field
/// would let a third-party keyboard in (paw also refuses them app-wide).
/// With Show on, the dots are drawn clear and the PIN is drawn over them.
/// It takes the focus on each PIN stage, shakes after a refused PIN, and
/// shows the PIN with one try left, so a slip is seen before it counts.
struct KeyPinField: View {
    let stage: KeySheetStage
    @Binding var pin: String
    let onContinue: () -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var showPin = false
    @State private var shake: CGFloat = 0
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: PawMetrics.Spacing.s) {
            SecureField("Key PIN", text: $pin)
                .focused($focused)
                .font(PawFont.password)
                .foregroundStyle(showPin ? Color.clear : PawColor.label)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(.asciiCapable)
                .submitLabel(.continue)
                .onSubmit {
                    // The button waits for 6 characters; the return key sends what
                    // was typed, and the core refuses a short PIN without using a try.
                    if !pin.isEmpty { onContinue() }
                }
                .accessibilityLabel("Key PIN")
                .accessibilityIdentifier("pin-field")
                // After the field's own accessibility, so the shown PIN is
                // its own element.
                .overlay(alignment: .leading) {
                    if showPin {
                        Text(pin)
                            .font(PawFont.password)
                            .foregroundStyle(PawColor.label)
                            .lineLimit(1)
                            .allowsHitTesting(false)
                            .accessibilityLabel(pin)
                            .accessibilityIdentifier("pin-shown")
                    }
                }

            Button(showPin ? "Hide" : "Show") {
                showPin.toggle()
            }
            .buttonStyle(.borderless)
            .font(PawFont.subheadline.weight(.semibold))
            .foregroundStyle(PawColor.label)
            .frame(minWidth: PawMetrics.tapMin, minHeight: PawMetrics.tapMin)
            .accessibilityLabel(showPin ? "Hide PIN" : "Show PIN")
            .accessibilityIdentifier("pin-show")
        }
        .padding(.leading, PawMetrics.Spacing.l)
        .padding(.trailing, PawMetrics.Spacing.s)
        .frame(minHeight: PawMetrics.control)
        .background(
            stage.followsRefusedPin ? PawColor.dangerFill : PawColor.fill,
            in: RoundedRectangle(cornerRadius: PawMetrics.inlineRadius, style: .continuous))
        .modifier(ShakeEffect(animatableData: shake))
        .onChange(of: stage, initial: true) {
            if stage.triesLeft == 1 { showPin = true }
            focused = true
            if stage.followsRefusedPin, !reduceMotion {
                withAnimation(.linear(duration: 0.4)) { shake += 1 }
            }
        }
    }
}

/// Plug in, PIN, touch: finished steps carry a check, the current one is inked.
struct KeyStepPills: View {
    /// Index of the step in progress: 0 plug in, 1 PIN, 2 touch, 3 all finished.
    let current: Int

    private static let steps: [(todo: String, done: String)] = [
        ("Plug in", "Plugged in"),
        ("PIN", "PIN correct"),
        ("Touch", "Touched"),
    ]

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: PawMetrics.Spacing.s) { pills }
            VStack(spacing: PawMetrics.Spacing.s) { pills }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(spokenLabel)
        .accessibilityIdentifier("key-sheet-steps")
    }

    private var pills: some View {
        ForEach(Array(Self.steps.enumerated()), id: \.offset) { index, step in
            HStack(spacing: 6) {
                if index < current {
                    Image(systemName: PawSymbol.stepDone)
                        .font(PawFont.caption.weight(.bold))
                }
                Text(index < current ? step.done : step.todo)
                    .font(index == current ? PawFont.footnote.weight(.semibold) : PawFont.footnote)
                    .lineLimit(1)
            }
            .foregroundStyle(index == current ? PawColor.onAction : PawColor.labelSecondary)
            .padding(.horizontal, PawMetrics.Spacing.m)
            .frame(minHeight: PawMetrics.pill)
            .background(index == current ? PawColor.action : PawColor.fill, in: Capsule())
        }
    }

    /// Says the steps already done as well, as the pills show them: at
    /// Touch with the PIN held from earlier, "PIN correct" tells the person
    /// it was given before, not skipped.
    private var spokenLabel: String {
        guard current < Self.steps.count else { return "All 3 steps finished" }
        let now = "Step \(current + 1) of \(Self.steps.count): \(Self.steps[current].todo)"
        let done = Self.steps.prefix(current).map(\.done)
        return done.isEmpty ? now : now + ". Done: " + done.joined(separator: ", ")
    }
}

/// The touch prompt: an outline of the key with its gold contact breathing.
/// It is a drawing, not a button; the disc alone is never the touch prompt.
private struct KeyDrawing: View {
    private let bodyWidth: CGFloat = 112
    private let bodyHeight: CGFloat = 146
    private let plugWidth: CGFloat = 40
    private let plugHeight: CGFloat = 14

    var body: some View {
        ZStack {
            VStack(spacing: -2) {
                RoundedRectangle(cornerRadius: 34, style: .continuous)
                    .strokeBorder(PawColor.labelTertiary, lineWidth: 2)
                    .frame(width: bodyWidth, height: bodyHeight)
                UnevenRoundedRectangle(
                    bottomLeadingRadius: 5, bottomTrailingRadius: 5, style: .continuous)
                    .strokeBorder(PawColor.labelTertiary, lineWidth: 2)
                    .frame(width: plugWidth, height: plugHeight)
            }
            KeyDisc(size: .hero, phase: .waiting)
                .offset(y: -plugHeight / 2 + 1)
        }
        .frame(width: PawMetrics.discHero * KeyDisc.haloReach * 1.04, height: bodyHeight + plugHeight - 2)
        .contentShape(Rectangle())
    }
}

/// Driven by a counter: each whole step is one shake.
private struct ShakeEffect: GeometryEffect {
    var travel: CGFloat = 8
    var swings: CGFloat = 3
    var animatableData: CGFloat

    func effectValue(size: CGSize) -> ProjectionTransform {
        ProjectionTransform(CGAffineTransform(
            translationX: travel * sin(animatableData * .pi * 2 * swings), y: 0))
    }
}
