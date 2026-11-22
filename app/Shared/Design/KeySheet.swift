import SwiftUI

/// Where the one key sheet is. Plain values only: the core decides the stage.
enum KeySheetStage: Equatable {
    case plugIn
    /// `triesLeft` is shown before any input whenever fewer than 3 remain.
    /// `forgotten` adds the line about the PIN being dropped in the background.
    case pin(triesLeft: Int, forgotten: Bool)
    /// The PIN just entered was wrong.
    case wrongPin(triesLeft: Int)
    /// The key is finishing something else.
    case busy
    /// The key is blinking and waiting for a touch.
    case touch
    case working(message: String, progress: String?)
    /// Touch received. The host closes the sheet after a second.
    case done
    /// Out of PIN tries.
    case locked
}

/// The content of the key sheet for every stage. The host presents it with
/// `.presentationDetents([.height(PawMetrics.keySheetHeight), .large])`.
struct KeySheetContent: View {
    /// What the key is for, e.g. "Open Fastmail".
    let purpose: String
    let stage: KeySheetStage
    @Binding var pin: String
    /// A line under the steps, e.g. "Afterwards it opens with Face ID until 6:40 PM."
    var note: String? = nil
    var minimumPinLength = 6
    var onContinue: () -> Void = {}
    var onCancel: () -> Void = {}

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var showPin = false
    @State private var shake: CGFloat = 0
    @State private var showTouchHint = false
    @FocusState private var focus: PinFocus?

    private enum PinFocus {
        case secure, plain
    }

    var body: some View {
        ScrollView {
            VStack(spacing: PawMetrics.Spacing.l) {
                Text(purpose)
                    .font(PawFont.sectionHeader)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("key-sheet-purpose")
                hero
                words
                if isPinStage {
                    pinField
                }
                if stage != .locked && stage != .done {
                    KeyStepPills(current: currentStep)
                }
                if let note, stage == .touch {
                    Text(note)
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: PawMetrics.sheetTextWidth)
                        .accessibilityIdentifier("key-sheet-note")
                }
                buttons
            }
            .padding(.horizontal, PawMetrics.Spacing.xl)
            .padding(.top, PawMetrics.Spacing.xl)
            .padding(.bottom, PawMetrics.Spacing.s)
            .frame(maxWidth: .infinity)
        }
        .scrollBounceBehavior(.basedOnSize)
        .onChange(of: stage, initial: true) {
            stageChanged()
        }
        .onChange(of: showPin) {
            if focus != nil {
                focus = showPin ? .plain : .secure
            }
        }
        .sensoryFeedback(trigger: stage) { _, new in
            Self.haptic(for: new)
        }
    }

    // MARK: Parts

    @ViewBuilder private var hero: some View {
        switch stage {
        case .plugIn, .busy:
            KeyDisc(size: .hero)
        case .pin, .wrongPin:
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
        }
    }

    private var words: some View {
        VStack(spacing: 6) {
            Text(headline)
                .font(PawFont.title)
                .foregroundStyle(PawColor.label)
                .multilineTextAlignment(.center)
                .accessibilityAddTraits(.isHeader)
                .accessibilityIdentifier("key-sheet-headline")
            if let triesLine {
                Label(triesLine, systemImage: PawSymbol.warning)
                    .font(PawFont.numeral)
                    .foregroundStyle(PawColor.warning)
                    .multilineTextAlignment(.center)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(triesLine)
                    .accessibilityIdentifier("pin-tries-left")
            }
            if let explanation {
                Text(explanation)
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

    private var pinField: some View {
        HStack(spacing: PawMetrics.Spacing.s) {
            Group {
                if showPin {
                    TextField("Key PIN", text: $pin)
                        .focused($focus, equals: .plain)
                } else {
                    SecureField("Key PIN", text: $pin)
                        .focused($focus, equals: .secure)
                }
            }
            .font(PawFont.password)
            .foregroundStyle(PawColor.label)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .keyboardType(.asciiCapable)
            .submitLabel(.continue)
            .onSubmit {
                if canContinue { onContinue() }
            }
            .accessibilityLabel("Key PIN")
            .accessibilityIdentifier("pin-field")

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
            isWrongPin ? PawColor.dangerFill : PawColor.fill,
            in: RoundedRectangle(cornerRadius: PawMetrics.inlineRadius, style: .continuous))
        .modifier(ShakeEffect(animatableData: shake))
    }

    @ViewBuilder private var buttons: some View {
        VStack(spacing: PawMetrics.Spacing.s) {
            switch stage {
            case .pin, .wrongPin:
                Button(triesLeft == 1 ? "Use last try" : "Continue", action: onContinue)
                    .buttonStyle(.pawPrimary)
                    .disabled(!canContinue)
                    .accessibilityIdentifier("pin-continue")
                cancelButton("Cancel")
            case .locked:
                cancelButton("Close")
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
            .accessibilityIdentifier("key-sheet-cancel")
    }

    // MARK: Words

    private var headline: String {
        switch stage {
        case .plugIn: "Plug in your key"
        case .pin: "Enter your key's PIN"
        case .wrongPin: "Wrong PIN"
        case .busy: "One moment"
        case .touch: "Touch your key"
        case .working(let message, _): message
        case .done: "Done"
        case .locked: "Your key is locked"
        }
    }

    private var explanation: String? {
        switch stage {
        case .plugIn:
            return "USB-C, on the bottom of your iPhone."
        case .pin(_, let forgotten):
            let usual = "The PIN for the key, not your iPhone passcode. Asked once while the key stays plugged in."
            return forgotten ? "paw forgot your PIN after 15 minutes away. " + usual : usual
        case .wrongPin:
            return "Enter the PIN for the key, not your iPhone passcode."
        case .busy:
            return "Your key is finishing something else."
        case .touch:
            return "Touch the gold part of your key. It's blinking."
        case .working(_, let progress):
            return progress
        case .done:
            return nil
        case .locked:
            return "Unlock it with the key's reset code or Admin PIN. Passwords that open with Face ID still work."
        }
    }

    private var triesLeft: Int? {
        switch stage {
        case .pin(let triesLeft, _), .wrongPin(let triesLeft): return triesLeft
        default: return nil
        }
    }

    private static func haptic(for stage: KeySheetStage) -> SensoryFeedback? {
        switch stage {
        case .wrongPin(let triesLeft): return triesLeft == 1 ? .warning : .error
        case .done: return .success
        default: return nil
        }
    }

    /// Shown before any input whenever fewer than 3 tries remain.
    private var triesLine: String? {
        guard let triesLeft, triesLeft < 3 else { return nil }
        if triesLeft == 1 {
            return "1 try left before the key locks."
        }
        return isWrongPin ? "\(triesLeft) tries left before the key locks." : "\(triesLeft) tries left."
    }

    private var announcement: String {
        [headline, triesLine, explanation].compactMap { $0 }.joined(separator: ". ")
            .replacingOccurrences(of: ".. ", with: ". ")
    }

    // MARK: State

    private var isPinStage: Bool {
        triesLeft != nil
    }

    private var isWrongPin: Bool {
        if case .wrongPin = stage { return true }
        return false
    }

    private var canContinue: Bool {
        pin.count >= minimumPinLength
    }

    private var currentStep: Int {
        switch stage {
        case .plugIn: 0
        case .pin, .wrongPin, .locked: 1
        case .busy, .touch: 2
        case .working, .done: 3
        }
    }

    private func stageChanged() {
        showTouchHint = false
        if isPinStage {
            // With one try left the PIN is shown, so a slip is seen before it counts.
            if triesLeft == 1 { showPin = true }
            focus = showPin ? .plain : .secure
        } else {
            focus = nil
        }
        if isWrongPin, !reduceMotion {
            withAnimation(.linear(duration: 0.4)) { shake += 1 }
        }
        AccessibilityNotification.Announcement(announcement).post()
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

    private var spokenLabel: String {
        guard current < Self.steps.count else { return "All 3 steps finished" }
        return "Step \(current + 1) of \(Self.steps.count): \(Self.steps[current].todo)"
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

/// Three swings of 8pt. Driven by a counter: each whole step is one shake.
private struct ShakeEffect: GeometryEffect {
    var travel: CGFloat = 8
    var swings: CGFloat = 3
    var animatableData: CGFloat

    func effectValue(size: CGSize) -> ProjectionTransform {
        ProjectionTransform(CGAffineTransform(
            translationX: travel * sin(animatableData * .pi * 2 * swings), y: 0))
    }
}
