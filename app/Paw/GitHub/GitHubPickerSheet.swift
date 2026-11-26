import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// "Choose from GitHub" under the address field: signs in with a code,
/// lists the repositories paw can see, and gives the chosen one's address
/// to setup, which goes on to the host check as if it had been typed.
struct GitHubChooseButton: View {
    let model: SetupModel
    let gitHub: GitHubAccess
    @State private var picker: GitHubPicker?

    var body: some View {
        Button("Choose from GitHub") {
            let picker = GitHubPicker(access: gitHub, purpose: .pick(chosen: { address in
                self.picker = nil
                model.addressText = address
                model.addressProblem = nil
                model.connectToHost()
            }))
            self.picker = picker
            picker.begin()
        }
        .buttonStyle(.pawSecondary)
        .disabled(model.working != nil)
        .accessibilityIdentifier("address-github")
        .pawSheet(item: $picker) { picker in
            GitHubPickerSheet(picker: picker)
        }
    }
}

/// The sign-in and the list, one sheet. Cancel or a swipe ends a sign-in
/// that is still waiting.
struct GitHubPickerSheet: View {
    let picker: GitHubPicker
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            content
                .navigationTitle(title)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        if case .code = picker.stage {
                            // Ends the wait; the sheet offers Try again.
                            Button("Cancel") { picker.cancelSignIn() }
                                .accessibilityIdentifier("github-cancel")
                        } else {
                            Button("Close") {
                                picker.dismissed()
                                dismiss()
                            }
                            .accessibilityIdentifier("github-close")
                        }
                    }
                }
                .background(PawColor.bg.ignoresSafeArea())
        }
        #if DEBUG && targetEnvironment(simulator)
        .simEndsFirstSignIn(picker)
        #endif
        .onDisappear { picker.dismissed() }
        .privacyCover()
    }

    private var title: String {
        switch picker.stage {
        case .starting, .code, .ended: "Sign in to GitHub"
        default: "Choose your store"
        }
    }

    @ViewBuilder private var content: some View {
        switch picker.stage {
        case .starting:
            GitHubWaitingView(text: nil)
        case .code(let code):
            GitHubCodeView(picker: picker, code: code)
        case .ended(let ending):
            GitHubProblemView(text: ending.words, identifier: "github-ended", actionTitle: "Try again") {
                picker.startSignIn()
            }
        case .listing:
            GitHubWaitingView(text: "Loading your repositories\u{2026}")
        case .choosing(let name):
            GitHubWaitingView(text: name)
        case .repositories(let repositories, let installUrl):
            GitHubRepositoryList(picker: picker, repositories: repositories, installUrl: installUrl)
        case .notInstalled(let installUrl, let storeMissing):
            GitHubNotInstalledView(picker: picker, installUrl: installUrl, storeMissing: storeMissing)
        case .listFailed(let words, let signInAgain):
            GitHubProblemView(
                text: words, identifier: "github-list-problem", actionTitle: signInAgain ? "Sign in again" : "Try again"
            ) {
                if signInAgain { picker.startSignIn() } else { picker.list() }
            }
        }
    }
}

// MARK: The code

private struct GitHubCodeView: View {
    let picker: GitHubPicker
    let code: DeviceCode
    @Environment(\.openURL) private var openURL
    @State private var copied = false
    @State private var opened = false

    /// This device only, gone when the code expires. Not a secret, so not
    /// `Clipboard`, which clears on Lock now.
    private func copyCode() {
        UIPasteboard.general.setItems(
            [[UTType.utf8PlainText.identifier: code.userCode]],
            options: [.localOnly: true, .expirationDate: Date().addingTimeInterval(TimeInterval(code.expiresIn))])
    }

    static let gitHubDevicePage = "https://github.com/login/device"

    /// The page to open: GitHub's own page carries the code (GitHub may
    /// ignore it); any other address opens exactly as the core gave it.
    static func pageURL(_ code: DeviceCode) -> URL? {
        guard code.verificationUri == gitHubDevicePage else { return URL(string: code.verificationUri) }
        var components = URLComponents()
        components.scheme = "https"
        components.host = "github.com"
        components.path = "/login/device"
        // Letters, digits and "-" as they are; anything else encoded.
        let plain = CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-")
        guard let value = code.userCode.addingPercentEncoding(withAllowedCharacters: plain) else { return nil }
        components.percentEncodedQueryItems = [URLQueryItem(name: "user_code", value: value)]
        return components.url
    }

    var body: some View {
        ScrollView {
            VStack(spacing: PawMetrics.Spacing.l) {
                Text("Enter this code on GitHub")
                    .font(PawFont.title)
                    .foregroundStyle(PawColor.label)
                    .multilineTextAlignment(.center)
                    .accessibilityAddTraits(.isHeader)
                Text(code.userCode)
                    .font(.system(.largeTitle, design: .monospaced, weight: .semibold))
                    .foregroundStyle(PawColor.label)
                    .textSelection(.enabled)
                    .padding(.vertical, PawMetrics.Spacing.l)
                    .frame(maxWidth: .infinity)
                    .background(PawColor.surface, in: RoundedRectangle(cornerRadius: PawMetrics.cardRadius, style: .continuous))
                    .accessibilityLabel("Code \(code.userCode.map(String.init).joined(separator: " "))")
                    .accessibilityValue(code.userCode)
                    .accessibilityIdentifier("github-code")
                VStack(spacing: PawMetrics.Spacing.s) {
                    Button {
                        copyCode()
                        opened = true
                        #if !(DEBUG && targetEnvironment(simulator))
                        // The simulated GitHub has no page to open.
                        if let url = Self.pageURL(code) { openURL(url) }
                        #endif
                    } label: {
                        Text("Open \(Self.shown(code.verificationUri))")
                    }
                    .buttonStyle(.pawPrimary)
                    .accessibilityIdentifier("github-open")
                    if opened {
                        Text("Code copied. Paste it on GitHub.")
                            .font(PawFont.footnote)
                            .foregroundStyle(PawColor.labelSecondary)
                            .multilineTextAlignment(.center)
                            .accessibilityIdentifier("github-copied")
                    }
                    Button(copied ? "Copied" : "Copy code") {
                        copyCode()
                        copied = true
                    }
                    .buttonStyle(.pawSecondary)
                    .accessibilityIdentifier("github-copy")
                }
                HStack(spacing: PawMetrics.Spacing.s) {
                    ProgressView()
                    Text("Waiting for GitHub\u{2026}")
                        .font(PawFont.subheadline)
                        .foregroundStyle(PawColor.labelSecondary)
                }
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("github-waiting")
                Text("Reads the store you choose. Never sends changes.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                #if DEBUG && targetEnvironment(simulator)
                SimulatorActionButton(action: SimulatorAction(
                    title: "authorise the code", identifier: "sim-github-authorise") { SimGitHubLaunch.shared.authorise() })
                #endif
            }
            .frame(maxWidth: PawMetrics.readableWidth)
            .frame(maxWidth: .infinity)
            .padding(PawMetrics.Spacing.xl)
        }
        .accessibilityIdentifier("github-sign-in")
    }

    /// "github.com/login/device": the address without its scheme. The
    /// simulated GitHub's own address would mean nothing in a picture.
    static func shown(_ uri: String) -> String {
        #if DEBUG && targetEnvironment(simulator)
        return "github.com/login/device"
        #else
        var text = uri
        for scheme in ["https://", "http://"] where text.hasPrefix(scheme) {
            text.removeFirst(scheme.count)
        }
        return text
        #endif
    }
}

// MARK: Waiting and problems

private struct GitHubWaitingView: View {
    let text: String?

    var body: some View {
        VStack(spacing: PawMetrics.Spacing.m) {
            ProgressView()
            if let text {
                Text(text)
                    .font(PawFont.subheadline)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("github-working")
    }
}

private struct GitHubProblemView: View {
    let text: String
    let identifier: String
    let actionTitle: String
    let action: () -> Void

    var body: some View {
        VStack(spacing: PawMetrics.Spacing.l) {
            Text(text)
                .font(PawFont.body)
                .foregroundStyle(PawColor.label)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityIdentifier(identifier)
            Button(actionTitle, action: action)
                .buttonStyle(.pawPrimary)
                .accessibilityIdentifier("github-try-again")
        }
        .frame(maxWidth: PawMetrics.sheetTextWidth)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(PawMetrics.Spacing.xl)
    }
}

// MARK: The list

private struct GitHubRepositoryList: View {
    let picker: GitHubPicker
    let repositories: [GitHubRepository]
    let installUrl: String
    @Environment(\.openURL) private var openURL
    @State private var query = ""

    /// Past this many the list can be searched.
    static let searchFrom = 8

    /// In the core's order: most recently pushed first.
    private var shown: [GitHubRepository] {
        let text = query.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return repositories }
        return repositories.filter { $0.fullName.localizedCaseInsensitiveContains(text) }
    }

    var body: some View {
        if repositories.count > Self.searchFrom {
            list.searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: "Search repositories")
        } else {
            list
        }
    }

    private var list: some View {
        List {
            Section {
                ForEach(shown, id: \.id) { repository in
                    Button {
                        picker.choose(repository)
                    } label: {
                        GitHubRepositoryRow(repository: repository)
                    }
                    .accessibilityIdentifier("github-repo-\(repository.fullName)")
                }
                if shown.isEmpty {
                    Text("No repository matches \u{201C}\(query)\u{201D}.")
                        .font(PawFont.subheadline)
                        .foregroundStyle(PawColor.labelSecondary)
                        .accessibilityIdentifier("github-no-match")
                }
            } footer: {
                Button {
                    if let url = URL(string: installUrl) { openURL(url) }
                } label: {
                    Text("Not here? Choose which repositories paw can see")
                        .font(PawFont.footnote.weight(.semibold))
                        .foregroundStyle(PawColor.label)
                        .multilineTextAlignment(.leading)
                }
                .buttonStyle(.plain)
                .padding(.top, PawMetrics.Spacing.s)
                .accessibilityIdentifier("github-install-link")
            }
        }
        .listStyle(.insetGrouped)
        .accessibilityIdentifier("github-repositories")
    }
}

private struct GitHubRepositoryRow: View {
    let repository: GitHubRepository

    var body: some View {
        HStack(spacing: PawMetrics.Spacing.m) {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                Text(repository.name)
                    .font(PawFont.body)
                    .foregroundStyle(PawColor.label)
                Text(repository.owner)
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
            Spacer(minLength: PawMetrics.Spacing.s)
            if repository.private {
                Image(systemName: "lock.fill")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
        .frame(minHeight: PawMetrics.rowTwoLine)
        .contentShape(Rectangle())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
    }

    private var label: String {
        var words = "\(repository.name), \(repository.owner)"
        if repository.private { words += ", private" }
        return words
    }
}

private struct GitHubNotInstalledView: View {
    let picker: GitHubPicker
    let installUrl: String
    let storeMissing: Bool
    @Environment(\.openURL) private var openURL

    var body: some View {
        ScrollView {
            VStack(spacing: PawMetrics.Spacing.l) {
                Text(storeMissing
                     ? "paw can't see this store's repository yet. Choose it on GitHub."
                     : "paw can't see any of your repositories yet. Choose your store on GitHub.")
                    .font(PawFont.body)
                    .foregroundStyle(PawColor.label)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("github-not-installed")
                VStack(spacing: PawMetrics.Spacing.s) {
                    Button("Choose which repositories paw can see") {
                        if let url = URL(string: installUrl) { openURL(url) }
                    }
                    .buttonStyle(.pawPrimary)
                    .accessibilityIdentifier("github-install-link")
                    Button("Check again") { picker.list() }
                        .buttonStyle(.pawSecondary)
                        .accessibilityIdentifier("github-check-again")
                }
                #if DEBUG && targetEnvironment(simulator)
                SimulatorActionButton(action: SimulatorAction(
                    title: "choose the store on GitHub", identifier: "sim-github-install") { SimGitHubLaunch.shared.install() })
                #endif
            }
            .frame(maxWidth: PawMetrics.sheetTextWidth)
            .frame(maxWidth: .infinity)
            .padding(PawMetrics.Spacing.xl)
        }
    }
}
