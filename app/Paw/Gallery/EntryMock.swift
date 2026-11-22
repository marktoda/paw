#if DEBUG
import SwiftUI

/// An opened entry as in the Keyring 2 mock-up: header, Face ID window,
/// fields, the password with reveal, copy and large type. Nothing is really
/// copied; the buttons only show the confirmation.
struct EntryMock: View {
    @State private var isRevealed = false
    @State private var showsLargeType = false
    @State private var toast: String?
    @State private var windowOpen = true

    private let password = GallerySample.password

    var body: some View {
        List {
            Section {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.l) {
                    HStack(spacing: 14) {
                        IconChip(symbol: "envelope.fill", tint: FolderTint(folder: "email"), size: .large)
                        VStack(alignment: .leading, spacing: 0) {
                            Text("Fastmail")
                                .font(PawFont.title)
                                .foregroundStyle(PawColor.label)
                                .accessibilityAddTraits(.isHeader)
                                .accessibilityIdentifier("entry-title")
                            Text("email, changed 14 November")
                                .font(PawFont.subheadline)
                                .foregroundStyle(PawColor.labelSecondary)
                        }
                    }
                    .padding(.horizontal, PawMetrics.Spacing.xs)
                    if windowOpen {
                        FaceIDWindowBar(text: "Opens with Face ID until 6:40 PM") {
                            windowOpen = false
                        }
                    }
                }
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            }
            Section {
                FieldRow(label: "Username", value: "you@example.com") {
                    toast = "Username copied."
                }
                .pawListRow()
                PasswordField(
                    password: password,
                    isRevealed: $isRevealed,
                    onCopy: copyPassword,
                    onLargeType: { showsLargeType = true })
                    .pawListRow()
                FieldRow(label: "Website", value: "mail.example.com") {
                    toast = "Website copied."
                }
                .pawListRow()
            }
            Section {
                VStack(spacing: PawMetrics.Spacing.s) {
                    Button("Copy password", action: copyPassword)
                        .buttonStyle(.pawPrimary)
                        .accessibilityIdentifier("copy-password")
                    Button(isRevealed ? "Hide password" : "Show password") {
                        isRevealed.toggle()
                    }
                    .buttonStyle(.pawSecondary)
                    .accessibilityIdentifier("show-password")
                }
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            }
            Section {
                Text("Recovery address is the old one. App passwords are listed under email/Fastmail app passwords.")
                    .font(PawFont.body)
                    .foregroundStyle(PawColor.label)
            } header: {
                PawSectionHeader("Notes")
            } footer: {
                Text("A copied password clears from the clipboard after 45 seconds.")
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
        .listStyle(.insetGrouped)
        .contentMargins(.top, 0, for: .scrollContent)
        .environment(\.defaultMinListRowHeight, PawMetrics.tapMin)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Edit") {}
                    .accessibilityIdentifier("edit-button")
            }
        }
        .copyToast($toast)
        .sheet(isPresented: $showsLargeType) {
            VStack(spacing: 0) {
                ZStack {
                    Text("Password")
                        .font(PawFont.headline)
                        .foregroundStyle(PawColor.label)
                        .accessibilityAddTraits(.isHeader)
                    HStack {
                        Spacer()
                        Button("Done") { showsLargeType = false }
                            .font(PawFont.headline)
                            .foregroundStyle(PawColor.label)
                            .frame(minWidth: PawMetrics.tapMin, minHeight: PawMetrics.tapMin)
                            .accessibilityIdentifier("large-type-done")
                    }
                }
                .padding(.horizontal, PawMetrics.Spacing.l)
                .padding(.top, PawMetrics.Spacing.l)
                LargeTypeView(password: password)
            }
            .presentationDetents([.medium, .large])
        }
    }

    private func copyPassword() {
        toast = "Password copied. Clears in 45 seconds."
    }
}
#endif
