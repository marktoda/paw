import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// First-run setup (design.md 2.4): one screen per step, Back on all but
/// the first, the shared key sheet for the steps that need the key.
struct SetupFlowView: View {
    @Bindable var model: SetupModel
    @Environment(\.scenePhase) private var scenePhase

    /// Swiping the key sheet down is Cancel: the core task stops too.
    private var keySheet: Binding<SetupSheet?> {
        Binding(
            get: { model.sheet },
            set: { sheet in
                if sheet == nil { model.cancelSheet() }
            })
    }

    var body: some View {
        NavigationStack {
            screen
                .toolbar {
                    if showsBack {
                        ToolbarItem(placement: .topBarLeading) {
                            Button {
                                model.back()
                            } label: {
                                Label("Back", systemImage: "chevron.left")
                            }
                            .disabled(model.working != nil)
                            .accessibilityLabel("Back")
                            .accessibilityIdentifier("setup-back")
                        }
                    }
                }
                .navigationBarTitleDisplayMode(.inline)
                .animation(.default, value: model.screen)
        }
        .sheet(item: keySheet) { _ in
            SetupKeySheet(model: model)
        }
        .onChange(of: scenePhase) {
            switch scenePhase {
            case .background: model.keyPort.session()?.noteBackgrounded()
            case .active: model.foregrounded()
            default: break
            }
        }
        #if DEBUG && targetEnvironment(simulator)
        .background(alignment: .topLeading) {
            SetupTaskCount(count: model.pendingTasks)
        }
        #endif
        .privacyCover()
    }

    private var showsBack: Bool {
        switch model.screen {
        case .welcome, .done: false
        default: true
        }
    }

    @ViewBuilder private var screen: some View {
        switch model.screen {
        case .welcome:
            WelcomeScreen(model: model)
        case .plugIn:
            PlugInScreen(model: model)
        case .yourKey(let key):
            YourKeyScreen(model: model, key: key)
        case .address(let notice):
            AddressScreen(model: model, notice: notice)
        case .confirmHost(let host, let algorithm, let fingerprint):
            ConfirmHostScreen(model: model, host: host, algorithm: algorithm, fingerprint: fingerprint)
        case .keyNotAccepted(let host, let publicKey):
            KeyNotAcceptedScreen(model: model, host: host, publicKey: publicKey)
        case .download(let address):
            DownloadScreen(model: model, address: address)
        case .check(let entries):
            CheckScreen(model: model, entries: entries)
        case .done(let opened, let entries):
            DoneScreen(model: model, opened: opened, entries: entries)
        }
    }
}

// MARK: S0 Welcome

private struct WelcomeScreen: View {
    let model: SetupModel

    var body: some View {
        SetupPage(step: nil, title: "paw", titleFont: PawFont.homeTitle, identifier: "setup-welcome") {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.xl) {
                Text("Your pass store, opened with your YubiKey.")
                    .font(PawFont.columnTitle)
                    .foregroundStyle(PawColor.label)
                    .fixedSize(horizontal: false, vertical: true)
                Label {
                    Text("You'll need your YubiKey and its PIN.")
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.labelSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                } icon: {
                    Image(systemName: PawSymbol.plugIn)
                        .foregroundStyle(PawColor.labelSecondary)
                }
            }
            .padding(.top, PawMetrics.Spacing.s)
            if let problem = model.problem {
                SetupProblemView(problem: problem, act: { model.performProblemAction() }, offer: { model.performOffer() })
            }
        } buttons: {
            Button {
                model.begin()
            } label: {
                KeyButtonLabel(title: "Set up")
            }
            .buttonStyle(.pawPrimary)
            .accessibilityIdentifier("setup-start")
        }
    }
}

// MARK: S1 Your key

private struct PlugInScreen: View {
    let model: SetupModel

    var body: some View {
        SetupPage(
            step: 1, title: "Plug in your YubiKey",
            message: "No PIN yet. Setup continues once it's in.",
            identifier: "setup-plug-in", simulatorActions: simulatorActions
        ) {
            HStack {
                Spacer()
                KeyDisc(size: .hero, phase: .waiting)
                Spacer()
            }
            .padding(.vertical, PawMetrics.Spacing.l)
            if let working = model.working, !working.isEmpty {
                SetupWorkingLine(text: working)
            }
            if model.noSmartCards {
                ErrorRow(
                    kind: .error, problem: "This iPhone can't use USB keys",
                    nextStep: "paw needs an iPhone that can read a YubiKey over USB-C.", isBanner: true)
            } else if let attachProblem = model.attachProblem {
                ErrorRow(kind: .error, problem: "paw couldn't read your key", nextStep: attachProblem, isBanner: true)
            }
            if let problem = model.problem {
                SetupProblemView(problem: problem, act: { model.performProblemAction() }, offer: { model.performOffer() })
            }
        } buttons: {
            if model.attachProblem != nil, !model.noSmartCards {
                // Reading the key again needs it in, but no PIN or touch.
                Button {
                    model.retryAttach()
                } label: {
                    KeyButtonLabel(title: "Try again")
                }
                .buttonStyle(.pawPrimary)
                .disabled(model.working != nil)
                .accessibilityIdentifier("plug-in-retry")
            }
        }
        .onAppear { model.readKey() }
    }

    private var simulatorActions: [SimulatorAction] {
        #if targetEnvironment(simulator)
        if !model.keyPresent {
            return [SimulatorAction(title: "plug in the key", identifier: "simulated-plug-in") {
                model.simulatedPlugIn()
            }]
        }
        if model.problem != nil {
            return [SimulatorAction(title: "read the key again", identifier: "simulated-read-key") {
                model.readKey()
            }]
        }
        #endif
        return []
    }
}

private struct YourKeyScreen: View {
    let model: SetupModel
    let key: KeySummary

    var body: some View {
        SetupPage(
            step: 1, title: "Your key",
            message: "It has both keys paw needs.",
            identifier: "setup-your-key"
        ) {
            SetupCard {
                Text("YubiKey \u{00B7} serial \(spokenSerial(key.ident))")
                    .font(PawFont.headline)
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("key-serial")
                Divider()
                slot(
                    name: "Decryption key", ending: fingerprintEnding(key.decryptionFingerprint),
                    touch: key.decryptionTouch, identifier: "key-decryption")
                Divider()
                slot(
                    name: "SSH key", ending: model.sshFingerprint.map(fingerprintEnding),
                    touch: key.authenticationTouch, identifier: "key-ssh")
            }
            ForEach(warnings, id: \.self) { warning in
                ErrorRow(kind: .warning, problem: warning.problem, nextStep: warning.nextStep)
            }
            if let problem = model.problem {
                SetupProblemView(problem: problem, act: { model.performProblemAction() }, offer: { model.performOffer() })
            }
        } buttons: {
            Button("Continue") { model.continueFromKey() }
                .buttonStyle(.pawPrimary)
                .accessibilityIdentifier("key-continue")
        }
    }

    private func slot(name: String, ending: String?, touch: KeyTouch, identifier: String) -> some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
            Label {
                Text(name)
                    .font(PawFont.body)
                    .foregroundStyle(PawColor.label)
            } icon: {
                Image(systemName: PawSymbol.synced)
                    .foregroundStyle(PawColor.success)
            }
            Text([ending.map { "ends \($0)" }, touch.phrase].compactMap { $0 }.joined(separator: " \u{00B7} "))
                .font(PawFont.footnote.monospacedDigit())
                .foregroundStyle(PawColor.labelSecondary)
                .padding(.leading, 30)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(identifier)
    }

    private struct Warning: Hashable {
        let problem: String
        let nextStep: String
    }

    /// What a weaker touch policy means, said once per kind.
    private var warnings: [Warning] {
        var found: [Warning] = []
        let touches = [key.decryptionTouch, key.authenticationTouch]
        if touches.contains(.off) {
            found.append(Warning(
                problem: "This key works without a touch",
                nextStep: "Anyone with this iPhone, your key and its PIN could read everything."))
        }
        if touches.contains(.cached) {
            found.append(Warning(
                problem: "One touch lasts about 15 seconds",
                nextStep: "In that time a second password can open without another touch."))
        }
        return found
    }
}

// MARK: S2 Address

private struct AddressScreen: View {
    @Bindable var model: SetupModel
    let notice: String?
    @FocusState private var focused: Bool
    @Environment(\.gitHubAccess) private var gitHub

    var body: some View {
        SetupPage(
            step: 2, title: "Where is your password store?",
            // With GitHub the choice comes first and the typed address is
            // the other way; without it, the address is the only way.
            message: gitHub == nil ? "Its SSH address, under Code \u{203A} SSH on your git host." : nil,
            identifier: "setup-address", simulatorActions: simulatorActions
        ) {
            if gitHub != nil {
                GitHubChooseButton(model: model)
            }
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
                if gitHub != nil {
                    Text("Or type its SSH address")
                        .font(PawFont.footnote.weight(.semibold))
                        .foregroundStyle(PawColor.labelSecondary)
                        .padding(.top, PawMetrics.Spacing.s)
                        .accessibilityAddTraits(.isHeader)
                }
                HStack(spacing: PawMetrics.Spacing.s) {
                    // Vertical, so a long address wraps instead of being cut
                    // off. Return then types a newline: it is taken out and
                    // connects, as Go would.
                    TextField("git@github.com:you/password-store.git", text: $model.addressText, axis: .vertical)
                        .lineLimit(1...4)
                        .font(PawFont.body.monospaced())
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .submitLabel(.go)
                        .focused($focused)
                        .onChange(of: model.addressText) { _, text in
                            model.addressProblem = nil
                            guard text.contains(where: \.isNewline) else { return }
                            model.addressText = text.filter { !$0.isNewline }
                            connect()
                        }
                        .accessibilityLabel("Store address")
                        .accessibilityIdentifier("repo-address")
                    PasteButton(payloadType: String.self) { strings in
                        if let text = strings.first {
                            model.addressText = text.trimmingCharacters(in: .whitespacesAndNewlines)
                            model.addressProblem = nil
                        }
                    }
                    .labelStyle(.iconOnly)
                    .buttonBorderShape(.capsule)
                    // Ink, not the system blue: paw has no accent colour.
                    .tint(PawColor.action)
                    .accessibilityIdentifier("address-paste")
                }
                .padding(.leading, PawMetrics.Spacing.l)
                .padding(.trailing, PawMetrics.Spacing.s)
                .padding(.vertical, PawMetrics.Spacing.xs)
                .frame(minHeight: PawMetrics.control)
                .background(
                    model.addressProblem == nil ? PawColor.surface : PawColor.dangerFill,
                    in: RoundedRectangle(cornerRadius: PawMetrics.inlineRadius, style: .continuous))
                if let addressProblem = model.addressProblem {
                    Label(addressProblem, systemImage: PawSymbol.error)
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.danger)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityIdentifier("address-problem")
                } else {
                    if let reading {
                        Text(SetupModel.readingLine(reading))
                            .font(PawFont.footnote)
                            .foregroundStyle(PawColor.label)
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityIdentifier("address-reading")
                        if let readingNotice = reading.notice {
                            Text(readingNotice)
                                .font(PawFont.footnote)
                                .foregroundStyle(PawColor.labelSecondary)
                                .fixedSize(horizontal: false, vertical: true)
                                .accessibilityIdentifier("address-reading-notice")
                        }
                    }
                    if let notice, notice != reading?.notice {
                        Text(notice)
                            .font(PawFont.footnote)
                            .foregroundStyle(PawColor.labelSecondary)
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityIdentifier("address-notice")
                    }
                }
            }
            if let working = model.working, !working.isEmpty {
                SetupWorkingLine(text: working)
            }
            if let problem = model.problem {
                SetupProblemView(problem: problem, act: { model.performProblemAction() }, offer: { model.performOffer() })
            }
        } buttons: {
            Button("Connect") { connect() }
                .buttonStyle(.pawPrimary)
                .disabled(model.working != nil || model.addressText.trimmingCharacters(in: .whitespaces).isEmpty)
                .accessibilityIdentifier("address-connect")
        }
    }

    /// Where paw will connect, as the address reads now.
    private var reading: AddressReading? {
        model.addressReading
    }

    private var simulatorActions: [SimulatorAction] {
        #if targetEnvironment(simulator)
        if let simulator = model.simulator {
            return [SimulatorAction(title: "use the simulated host", identifier: "simulated-address") {
                model.addressText = simulator.address
                model.addressProblem = nil
            }]
        }
        #endif
        return []
    }

    private func connect() {
        guard model.working == nil, !model.addressText.trimmingCharacters(in: .whitespaces).isEmpty else { return }
        focused = false
        model.connectToHost()
    }
}

private struct ConfirmHostScreen: View {
    let model: SetupModel
    let host: String
    let algorithm: String
    let fingerprint: String

    var body: some View {
        SetupPage(
            step: 2, title: "First time connecting to \(host)",
            message: "Compare this fingerprint with the one from your server's admin.",
            identifier: "setup-confirm-host"
        ) {
            SetupCard {
                Text(algorithm)
                    .font(PawFont.footnote.weight(.semibold))
                    .foregroundStyle(PawColor.labelSecondary)
                GroupedFingerprint(fingerprint: fingerprint)
                    .accessibilityIdentifier("host-fingerprint")
            }
            Text("If it doesn't match, go back: someone may be intercepting the connection.")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
                .fixedSize(horizontal: false, vertical: true)
            if let working = model.working, !working.isEmpty {
                SetupWorkingLine(text: working)
            }
            if let problem = model.problem {
                SetupProblemView(problem: problem, act: { model.performProblemAction() }, offer: { model.performOffer() })
            }
        } buttons: {
            Button("Trust this server") { model.trustHost(fingerprint: fingerprint) }
                .buttonStyle(.pawPrimary)
                .disabled(model.working != nil)
                .accessibilityIdentifier("host-trust")
            Button("Back") { model.back() }
                .buttonStyle(.pawSecondary)
                .disabled(model.working != nil)
                .accessibilityIdentifier("host-back")
        }
    }
}

private struct KeyNotAcceptedScreen: View {
    let model: SetupModel
    let host: String
    let publicKey: String
    @State private var toast: String?

    var body: some View {
        SetupPage(
            step: 2, title: "\(host) doesn't know this key yet",
            message: "Add this SSH key to your account on \(host), then check again.",
            identifier: "setup-key-not-accepted", simulatorActions: simulatorActions
        ) {
            SetupCard {
                Text("SSH key")
                    .font(PawFont.footnote.weight(.semibold))
                    .foregroundStyle(PawColor.labelSecondary)
                Text(publicKey)
                    .font(PawFont.footnote.monospaced())
                    .foregroundStyle(PawColor.label)
                    .lineLimit(3)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
                    .accessibilityLabel("SSH key")
                    .accessibilityValue(publicKey)
                    .accessibilityIdentifier("ssh-public-key")
                HStack(spacing: PawMetrics.Spacing.s) {
                    Button {
                        // A public key, not a secret: it stays on this
                        // iPhone's pasteboard with no expiry.
                        UIPasteboard.general.setItems(
                            [[UTType.utf8PlainText.identifier: publicKey]], options: [.localOnly: true])
                        toast = "SSH key copied."
                    } label: {
                        Label("Copy", systemImage: PawSymbol.copy)
                    }
                    .accessibilityIdentifier("copy-ssh-key")
                    ShareLink(item: publicKey) {
                        Label("Share\u{2026}", systemImage: "square.and.arrow.up")
                    }
                    .accessibilityIdentifier("share-ssh-key")
                }
                .buttonStyle(.borderless)
                .font(PawFont.subheadline.weight(.semibold))
                .foregroundStyle(PawColor.label)
                .labelStyle(.titleAndIcon)
            }
            if host == "github.com", let url = URL(string: "https://github.com/settings/ssh/new") {
                Link(destination: url) {
                    Label("Open GitHub settings", systemImage: "arrow.up.right")
                        .font(PawFont.subheadline.weight(.semibold))
                        .foregroundStyle(PawColor.label)
                }
                .accessibilityIdentifier("open-github")
            }
            if let working = model.working, !working.isEmpty {
                SetupWorkingLine(text: working)
            }
            if let problem = model.problem {
                SetupProblemView(problem: problem, act: { model.performProblemAction() }, offer: { model.performOffer() })
            }
        } buttons: {
            Button("Check again") { model.checkAgain() }
                .buttonStyle(.pawPrimary)
                .disabled(model.working != nil)
                .accessibilityIdentifier("check-again")
        }
        .copyToast($toast)
    }

    private var simulatorActions: [SimulatorAction] {
        #if targetEnvironment(simulator)
        if let simulator = model.simulator {
            return [SimulatorAction(title: "add the key on the host", identifier: "simulated-accept-key") {
                simulator.acceptKey()
            }]
        }
        #endif
        return []
    }
}

// MARK: S3 Download

private struct DownloadScreen: View {
    let model: SetupModel
    let address: String

    var body: some View {
        SetupPage(
            step: 3, title: "Download your store",
            message: "\(SetupModel.host(of: address)) accepts your key.",
            identifier: "setup-download", simulatorActions: simulatorActions
        ) {
            SetupCard {
                Text(address)
                    .font(PawFont.footnote.monospaced())
                    .foregroundStyle(PawColor.label)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                    .accessibilityIdentifier("download-address")
                Text("Needs your key's PIN and a touch.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let problem = model.problem {
                SetupProblemView(problem: problem, act: { model.performProblemAction() }, offer: { model.performOffer() })
            }
        } buttons: {
            // After the host's identity changed, the only way on is to
            // start again: no button invites another try.
            if model.problem?.action != .startOver {
                Button {
                    model.startDownload()
                } label: {
                    KeyButtonLabel(title: "Download")
                }
                .buttonStyle(.pawPrimary)
                .disabled(model.working != nil)
                .accessibilityIdentifier("download-start")
            }
        }
    }

    private var simulatorActions: [SimulatorAction] {
        #if targetEnvironment(simulator)
        if model.touchLeftWaiting, model.sheet == nil {
            return [SimulatorAction(title: "time out the key's touch", identifier: "simulated-time-out-touch") {
                model.simulatedTouchTimeout()
            }]
        }
        #endif
        return []
    }
}

// MARK: S4 Check

private struct CheckScreen: View {
    let model: SetupModel
    let entries: UInt32

    var body: some View {
        SetupPage(
            step: 4, title: "Check that your key opens it",
            message: "One password is opened as a test. It isn't shown or kept.",
            identifier: "setup-check"
        ) {
            Label {
                Text(entries == 1 ? "Downloaded 1 password." : "Downloaded \(entries) passwords.")
                    .font(PawFont.body)
                    .foregroundStyle(PawColor.label)
            } icon: {
                Image(systemName: PawSymbol.synced)
                    .foregroundStyle(PawColor.success)
            }
            .accessibilityIdentifier("check-count")
            if let problem = model.problem {
                SetupProblemView(problem: problem, act: { model.performProblemAction() }, offer: { model.performOffer() })
            }
        } buttons: {
            Button {
                model.startCheck()
            } label: {
                KeyButtonLabel(title: "Check my key")
            }
            .buttonStyle(.pawPrimary)
            .disabled(model.working != nil)
            .accessibilityIdentifier("check-start")
        }
    }
}

// MARK: Done

private struct DoneScreen: View {
    let model: SetupModel
    let opened: String
    let entries: UInt32

    var body: some View {
        SetupPage(
            step: nil, title: "Your store is ready",
            message: "Your key opened \(opened).",
            identifier: "setup-done"
        ) {
            SetupCard {
                Label {
                    Text(summary)
                        .font(PawFont.body)
                        .foregroundStyle(PawColor.label)
                } icon: {
                    Image(systemName: PawSymbol.synced)
                        .foregroundStyle(PawColor.success)
                }
                .accessibilityIdentifier("done-count")
                if let readers = whoCanOpen {
                    Text(readers)
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityIdentifier("done-recipients")
                }
            }
            if let readiness = model.readiness, !readiness.canSave {
                ErrorRow(
                    kind: .warning,
                    problem: readiness.missingRecipients.count == 1
                        ? "This store is also encrypted for 1 other key"
                        : "This store is also encrypted for \(readiness.missingRecipients.count) other keys",
                    nextStep: "You can open everything. Adding or changing passwords needs the public key of "
                        + readiness.missingRecipients.map(Self.spokenRecipient).joined(separator: ", ") + ".",
                    isBanner: true)
            }
        } buttons: {
            Button("Open my passwords") { model.openStore() }
                .buttonStyle(.pawPrimary)
                .accessibilityIdentifier("done-open")
        }
    }

    private var summary: String {
        let passwords = entries == 1 ? "1 password" : "\(entries) passwords"
        guard let folders = model.storeFacts?.folders, folders > 0 else { return passwords }
        return "\(passwords) in \(folders == 1 ? "1 folder" : "\(folders) folders")"
    }

    /// Who can open the store's passwords, from its top-level `.gpg-id`.
    private var whoCanOpen: String? {
        guard let recipients = model.storeFacts?.recipients, recipients > 0 else { return nil }
        let others = recipients - 1
        switch others {
        case 0: return "Encrypted for your key only"
        case 1: return "Encrypted for your key and 1 other"
        default: return "Encrypted for your key and \(others) others"
        }
    }

    /// "key ending 2C11 41DE", or the line as written if it is not hex.
    private static func spokenRecipient(_ line: String) -> String {
        let hex = line.hasPrefix("0x") ? String(line.dropFirst(2)) : line
        guard hex.count >= 8, hex.allSatisfy(\.isHexDigit) else { return line }
        return "the key ending \(fingerprintEnding(hex))"
    }
}

// MARK: The key sheet

/// The shared key sheet, following `model.sheet`.
private struct SetupKeySheet: View {
    @Bindable var model: SetupModel
    @State private var detent = PresentationDetent.height(PawMetrics.keySheetHeight)

    private var stage: KeySheetStage {
        model.sheet?.stage ?? .busy
    }

    var body: some View {
        KeySheetContent(
            purpose: model.sheet?.purpose == .check ? "Check your key" : "Download your store",
            stage: stage,
            pin: $model.pinEntry,
            onContinue: { model.continueWithPin() },
            onCancel: { model.cancelSheet() },
            onRetry: { model.retrySheet() },
            simulatorAction: simulatorAction,
            moreSimulatorActions: moreSimulatorActions)
            #if DEBUG && targetEnvironment(simulator)
            .background(alignment: .topLeading) {
                SetupTaskCount(count: model.pendingTasks)
            }
            #endif
            .presentationDetents([.height(PawMetrics.keySheetHeight), .large], selection: $detent)
            .presentationDragIndicator(.visible)
            .pawSheetBackground()
            .onChange(of: stage, initial: true) {
                detent = Self.needsKeyboard(stage) ? .large : .height(PawMetrics.keySheetHeight)
            }
            .privacyCover()
    }

    private var simulatorAction: SimulatorAction? {
        #if targetEnvironment(simulator)
        switch stage {
        case .touch:
            return SimulatorAction(title: "touch the key", identifier: "simulated-touch") {
                KeyConnection.simulateTouch()
            }
        case .plugIn:
            return SimulatorAction(title: "plug in the key", identifier: "simulated-plug-in") {
                model.simulatedPlugIn()
            }
        default:
            return nil
        }
        #else
        return nil
        #endif
    }

    private var moreSimulatorActions: [SimulatorAction] {
        #if targetEnvironment(simulator)
        if stage == .touch {
            return [SimulatorAction(title: "time out the touch", identifier: "simulated-touch-timeout") {
                KeyConnection.simulateTouchTimeout()
            }]
        }
        #endif
        return []
    }

    private static func needsKeyboard(_ stage: KeySheetStage) -> Bool {
        switch stage {
        case .pin, .wrongPin, .pinAgain: true
        default: false
        }
    }
}

#if DEBUG && targetEnvironment(simulator)
/// For the UI tests: how many core tasks have not ended, so a test can
/// wait for "0" before checking that something did not happen.
private struct SetupTaskCount: View {
    let count: Int

    var body: some View {
        KeyTasksSignal(tasks: count)
    }
}
#endif
