import SwiftUI

/// Settings › GitHub: whether paw is signed in and to which repository,
/// signing in and out. Shown for a store on GitHub, or once signed in.
struct GitHubSettingsSection: View {
    @Environment(\.gitHubAccess) private var gitHub
    @Environment(\.storeControls) private var controls
    @Environment(\.openURL) private var openURL
    @State private var picker: GitHubPicker?
    @State private var confirming = false
    @State private var signingOut = false
    @State private var report: SignOutReport?
    @State private var revokeAsked = false
    @State private var problem: String?

    var body: some View {
        if let gitHub, let controls, shows(gitHub, address: controls.address) {
            Section {
                status(gitHub)
                if gitHub.signedIn {
                    signOut(gitHub)
                } else {
                    Button("Sign in to GitHub") {
                        report = nil
                        let picker = GitHubPicker(access: gitHub, purpose: .link(address: controls.address, linked: {
                            self.picker = nil
                        }))
                        self.picker = picker
                        picker.begin()
                    }
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("settings-github-sign-in")
                }
                if let report {
                    signedOut(report)
                }
                if let problem {
                    ErrorRow(kind: .error, problem: "You're still signed in", nextStep: problem)
                }
                #if targetEnvironment(simulator)
                Button("Simulator: remove paw on GitHub") { SimGitHubLaunch.shared.revoke() }
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    .accessibilityIdentifier("sim-github-revoke")
                #endif
            } header: {
                PawSectionHeader("GitHub")
            } footer: {
                Text(gitHub.signedIn
                     ? "The sign-in can read this one repository, to check for changes without your key. It never sends changes."
                     : "Sign in to check for changes without your key. Sending changes always needs your key.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
    }

    private func shows(_ gitHub: GitHubAccess, address: String) -> Bool {
        #if targetEnvironment(simulator)
        return true
        #else
        return gitHub.signedIn || gitHub.link != nil || SetupModel.host(of: address).lowercased().hasSuffix("github.com")
        #endif
    }

    private func status(_ gitHub: GitHubAccess) -> some View {
        LabeledContent {
            Text(gitHub.signedIn ? (gitHub.link?.fullName ?? "No repository chosen") : "Not signed in")
                .foregroundStyle(PawColor.labelSecondary)
                .multilineTextAlignment(.trailing)
        } label: {
            Text(gitHub.signedIn ? "Signed in" : "Sign-in")
                .foregroundStyle(PawColor.label)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("settings-github-status")
        .sheet(item: $picker) { picker in
            GitHubPickerSheet(picker: picker)
        }
    }

    @ViewBuilder private func signOut(_ gitHub: GitHubAccess) -> some View {
        if confirming {
            Text("Checking for changes will need your key again.")
                .font(PawFont.subheadline)
                .foregroundStyle(PawColor.label)
                .accessibilityIdentifier("settings-github-sign-out-warning")
            Button("Sign out") { perform(gitHub, revoke: false) }
                .font(PawFont.body.weight(.semibold))
                .foregroundStyle(PawColor.danger)
                .disabled(signingOut)
                .accessibilityIdentifier("settings-github-sign-out-confirm")
            Button("Sign out and revoke on GitHub") { perform(gitHub, revoke: true) }
                .foregroundStyle(PawColor.danger)
                .disabled(signingOut)
                .accessibilityIdentifier("settings-github-sign-out-revoke")
            Button("Stay signed in") { confirming = false }
                .foregroundStyle(PawColor.label)
                .disabled(signingOut)
                .accessibilityIdentifier("settings-github-sign-out-cancel")
        } else {
            Button("Sign out of GitHub\u{2026}") {
                problem = nil
                confirming = true
            }
            .foregroundStyle(PawColor.danger)
            .accessibilityIdentifier("settings-github-sign-out")
        }
    }

    private func perform(_ gitHub: GitHubAccess, revoke: Bool) {
        signingOut = true
        revokeAsked = revoke
        gitHub.signOut(revoke: revoke) { result in
            signingOut = false
            confirming = false
            switch result {
            case .success(let done):
                report = done
                problem = nil
            case .failure(let error):
                problem = GitHubWords.problem(error)
            }
        }
    }

    @ViewBuilder private func signedOut(_ report: SignOutReport) -> some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.xs) {
            Text(revokeAsked && !report.revoked
                 ? "Signed out. GitHub didn't confirm the revoke."
                 : "Signed out.")
                .font(PawFont.subheadline)
                .foregroundStyle(PawColor.label)
            Text("To remove paw from your GitHub account, open \(report.manageUrl).")
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("settings-github-signed-out")
        Button("Open GitHub settings") {
            if let url = URL(string: report.manageUrl) { openURL(url) }
        }
        .foregroundStyle(PawColor.label)
        .accessibilityIdentifier("settings-github-manage")
    }
}
