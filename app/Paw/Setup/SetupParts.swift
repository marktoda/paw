import SwiftUI

/// One setup screen: the step counter, a rounded title, one or two
/// sentences, the screen's own content, an inline problem if there is one,
/// and its buttons pinned to the bottom, above the keyboard.
struct SetupPage<Content: View, Buttons: View>: View {
    let step: Int?
    let title: String
    var message: String? = nil
    var titleFont = PawFont.title
    let identifier: String
    /// The simulator's stand-ins for this screen, under its buttons.
    var simulatorActions: [SimulatorAction] = []
    @ViewBuilder var content: Content
    @ViewBuilder var buttons: Buttons

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.l) {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
                    if let step {
                        Text("Step \(step) of \(SetupScreen.stepCount)")
                            .font(PawFont.footnote.weight(.semibold))
                            .foregroundStyle(PawColor.labelSecondary)
                            .accessibilityIdentifier("setup-step")
                    }
                    Text(title)
                        .font(titleFont)
                        .foregroundStyle(PawColor.label)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityAddTraits(.isHeader)
                    if let message {
                        Text(message)
                            .font(PawFont.body)
                            .foregroundStyle(PawColor.labelSecondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                // The counter is read with the heading. The screen's name
                // rides on it, for tests.
                .accessibilityElement(children: .combine)
                .accessibilityAddTraits(.isHeader)
                .accessibilityIdentifier(identifier)
                content
            }
            .frame(maxWidth: PawMetrics.setupTextWidth, alignment: .leading)
            // On iPad the column sits in the middle of the screen.
            .frame(maxWidth: .infinity, alignment: PadLayout.isPad ? .center : .leading)
            .padding(.horizontal, PawMetrics.screenMargin)
            .padding(.top, PawMetrics.Spacing.s)
            .padding(.bottom, PawMetrics.Spacing.xl)
        }
        .scrollDismissesKeyboard(.interactively)
        .scrollBounceBehavior(.basedOnSize)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: PawMetrics.Spacing.s) {
                buttons
                SimulatorStrip(actions: simulatorActions)
            }
            .frame(maxWidth: PawMetrics.setupTextWidth)
            .padding(.horizontal, PawMetrics.screenMargin)
            .padding(.top, PawMetrics.Spacing.s)
            .padding(.bottom, PawMetrics.Spacing.l)
            // On the text's column, not centred beside it.
            .frame(maxWidth: .infinity, alignment: PadLayout.isPad ? .center : .leading)
            .background(PawColor.bg)
        }
        .background(PawColor.bg.ignoresSafeArea())
    }
}

extension PawMetrics {
    /// Running text in setup is at most 340pt wide (design-language 3).
    static let setupTextWidth: CGFloat = 340
}

/// A white card on the grouped background, for a key, a fingerprint or a
/// public key.
struct SetupCard<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.m) {
            content
        }
        .padding(PawMetrics.Spacing.l)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(PawColor.surface, in: RoundedRectangle(cornerRadius: PawMetrics.cardRadius, style: .continuous))
    }
}

#if targetEnvironment(simulator)
/// A stand-in for something outside paw (the key, the git host), on a
/// screen that takes it in its content. `SetupPage(simulatorActions:)`
/// puts them under the buttons instead.
struct SetupSimulatorButton: View {
    let title: String
    let identifier: String
    let perform: () -> Void

    var body: some View {
        SimulatorStrip(actions: [SimulatorAction(title: title, identifier: identifier, perform: perform)])
    }
}
#endif

/// A primary button whose tap asks for the key: the gold disc leads it
/// (design.md 2.1, rule 1).
struct KeyButtonLabel: View {
    let title: String

    var body: some View {
        HStack(spacing: PawMetrics.Spacing.s) {
            KeyDisc(size: .marker)
            Text(title)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title), needs your key")
    }
}

/// The inline problem: what happened, what to do, the lines to read
/// closely, its one action, and the other way on it offers, if any.
struct SetupProblemView: View {
    let problem: SetupProblem
    let act: () -> Void
    let offerAct: () -> Void

    init(problem: SetupProblem, act: @escaping () -> Void, offer: @escaping () -> Void = {}) {
        self.problem = problem
        self.act = act
        offerAct = offer
    }

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
            ErrorRow(
                kind: problem.kind, problem: problem.problem, nextStep: problem.nextStep,
                actionTitle: problem.actionTitle, action: act, isBanner: true)
            if !problem.details.isEmpty {
                DisclosureGroup("Details") {
                    VStack(alignment: .leading, spacing: PawMetrics.Spacing.xs) {
                        ForEach(problem.details, id: \.self) { line in
                            Text(line)
                                .font(PawFont.footnote.monospaced())
                                .foregroundStyle(PawColor.labelSecondary)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.top, PawMetrics.Spacing.xs)
                }
                .font(PawFont.footnote.weight(.semibold))
                .foregroundStyle(PawColor.labelSecondary)
                .tint(PawColor.labelSecondary)
                .padding(.horizontal, PawMetrics.Spacing.l)
                .accessibilityIdentifier("setup-problem-details")
            }
            if let offer = problem.offer {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
                    if let text = offer.text {
                        Text(text)
                            .font(PawFont.subheadline)
                            .foregroundStyle(PawColor.label)
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityIdentifier("\(offer.identifier)-text")
                    }
                    Button(offer.title, action: offerAct)
                        .buttonStyle(.borderless)
                        .font(PawFont.subheadline.weight(.semibold))
                        .foregroundStyle(PawColor.label)
                        .padding(.horizontal, PawMetrics.Spacing.m)
                        .frame(minHeight: PawMetrics.pill)
                        .background(PawColor.fill, in: Capsule())
                        .accessibilityIdentifier(offer.identifier)
                }
                .padding(.horizontal, PawMetrics.Spacing.l)
            }
        }
    }
}

/// A quiet "working" line under a screen's content.
struct SetupWorkingLine: View {
    let text: String

    var body: some View {
        HStack(spacing: PawMetrics.Spacing.s) {
            ProgressView()
            Text(text)
                .font(PawFont.subheadline)
                .foregroundStyle(PawColor.labelSecondary)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("setup-working")
    }
}

/// A fingerprint broken into groups of four so it can be read aloud and
/// compared: "SHA256:" then "uNiV ztFv …".
struct GroupedFingerprint: View {
    let fingerprint: String

    var body: some View {
        Text(Self.grouped(fingerprint))
            .font(PawFont.password)
            .foregroundStyle(PawColor.label)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityLabel(fingerprint)
    }

    static func grouped(_ fingerprint: String) -> String {
        var prefix = ""
        var hash = Substring(fingerprint)
        if let colon = fingerprint.firstIndex(of: ":") {
            prefix = String(fingerprint[...colon]) + "\n"
            hash = fingerprint[fingerprint.index(after: colon)...]
        }
        let groups = stride(from: 0, to: hash.count, by: 4).map { offset -> String in
            let start = hash.index(hash.startIndex, offsetBy: offset)
            let end = hash.index(start, offsetBy: 4, limitedBy: hash.endIndex) ?? hash.endIndex
            return String(hash[start..<end])
        }
        return prefix + groups.joined(separator: " ")
    }
}

extension KeyTouch {
    /// How a slot's touch policy reads on "Your key".
    var phrase: String {
        switch self {
        case .eachUse: "touch every time"
        case .cached: "one touch lasts about 15 seconds"
        case .off: "no touch needed"
        case .unknown: "may need a touch"
        }
    }
}

/// "12 345 678" from an ident such as "0006:12345678".
func spokenSerial(_ ident: String) -> String {
    let serial = ident.split(separator: ":").last.map(String.init) ?? ident
    var groups: [String] = []
    var rest = Substring(serial)
    while rest.count > 3 {
        groups.insert(String(rest.suffix(3)), at: 0)
        rest = rest.dropLast(3)
    }
    if !rest.isEmpty {
        groups.insert(String(rest), at: 0)
    }
    return groups.joined(separator: " ")
}

/// "2C11 41DE": the last eight hex digits of a fingerprint, in two groups.
func fingerprintEnding(_ fingerprint: String) -> String {
    let hex = fingerprint.filter(\.isHexDigit).uppercased()
    let tail = hex.suffix(8)
    guard tail.count == 8 else { return String(tail) }
    return "\(tail.prefix(4)) \(tail.suffix(4))"
}
