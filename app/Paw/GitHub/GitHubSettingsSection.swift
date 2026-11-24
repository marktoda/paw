import SwiftUI

/// Settings › GitHub: whether paw is signed in and to which store,
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
                if gitHub.signedIn {
                    status(gitHub)
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
            } header: {
                PawSectionHeader("GitHub")
            } footer: {
                Text("Reads this store only. Sending still needs your key.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                    // Here, not on the button: signing in swaps the button
                    // for the status row while the sheet is still choosing.
                    .sheet(item: $picker) { picker in
                        GitHubPickerSheet(picker: picker)
                    }
            }
            #if DEBUG && targetEnvironment(simulator)
            // Debug only, in its own section so no screenshot reads it as
            // part of the card.
            Section {
                Button {
                    SimGitHubLaunch.shared.revoke()
                } label: {
                    Label("Simulator: remove paw on GitHub", systemImage: "hammer")
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
                .accessibilityIdentifier("sim-github-revoke")
            }
            #endif
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
            Text(gitHub.link?.fullName ?? "No store chosen")
                .foregroundStyle(PawColor.labelSecondary)
                .multilineTextAlignment(.trailing)
        } label: {
            Text("Signed in")
                .foregroundStyle(PawColor.label)
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("settings-github-status")
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
            Button("Sign out and end it on GitHub") { perform(gitHub, revoke: true) }
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
        Text(report.revoked
             ? "Signed out. GitHub ended the sign-in."
             : revokeAsked ? "Signed out. GitHub didn't confirm it ended the sign-in." : "Signed out.")
            .font(PawFont.subheadline)
            .foregroundStyle(PawColor.label)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("settings-github-signed-out")
        // Ending the sign-in leaves paw listed on the account.
        Button("Remove paw on GitHub") {
            if let url = URL(string: report.manageUrl) { openURL(url) }
        }
        .foregroundStyle(PawColor.label)
        .accessibilityIdentifier("settings-github-manage")
    }
}
