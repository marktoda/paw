import SwiftUI

/// One setup screen: its step and heading, its content, then the step
/// under way and the problem it ran into, both from the model, with the
/// buttons at the bottom.
struct SetupPage<Content: View, Buttons: View>: View {
    let model: SetupModel
    let title: String
    var message: String? = nil
    var titleFont = PawFont.title
    let identifier: String
    @ViewBuilder var content: Content
    @ViewBuilder var buttons: Buttons
    #if DEBUG && targetEnvironment(simulator)
    @Environment(\.simulatorActions) private var simulatorActions
    #endif

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.l) {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
                    if let step = model.screen.step {
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
                if let line = model.working?.line {
                    SetupWorkingLine(text: line)
                }
                if let problem = model.problem {
                    SetupProblemView(
                        problem: problem, act: { model.performProblemAction() }, offerAct: { model.performOffer() })
                }
            }
            .frame(maxWidth: PawMetrics.setupTextWidth, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: PadLayout.isPad ? .center : .leading)
            .padding(.horizontal, PawMetrics.screenMargin)
            .padding(.top, PawMetrics.Spacing.s)
            .padding(.bottom, PawMetrics.Spacing.xl)
        }
        .scrollDismissesKeyboard(.interactively)
        .scrollBounceBehavior(.basedOnSize)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: PawMetrics.Spacing.s) {
                // One step at a time: none starts while another is under way.
                buttons
                    .disabled(model.busy)
                #if DEBUG && targetEnvironment(simulator)
                SimulatorStrip(actions: simulatorActions)
                #endif
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

struct SetupProblemView: View {
    let problem: SetupProblem
    let act: () -> Void
    let offerAct: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
            ErrorRow(
                kind: .error, problem: problem.problem, nextStep: problem.nextStep,
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

extension KeyTouch {
    var phrase: String {
        switch self {
        case .eachUse: "touch every time"
        case .cached: "one touch lasts about 15 seconds"
        case .off: "no touch needed"
        case .unknown: "may need a touch"
        }
    }
}
