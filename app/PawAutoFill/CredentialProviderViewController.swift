import AuthenticationServices
import SwiftUI

/// The AutoFill extension, until filling is built (design 2.10): it says
/// so and closes. It asks for no PIN and opens no connection to the key.
final class CredentialProviderViewController: ASCredentialProviderViewController {
    override func viewDidLoad() {
        super.viewDidLoad()

        let host = UIHostingController(rootView: ComingSoon { [weak self] in
            self?.extensionContext.cancelRequest(
                withError: NSError(
                    domain: ASExtensionErrorDomain,
                    code: ASExtensionError.userCanceled.rawValue))
        })
        addChild(host)
        host.view.frame = view.bounds
        host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(host.view)
        host.didMove(toParent: self)
    }

    override func prepareCredentialList(for serviceIdentifiers: [ASCredentialServiceIdentifier]) {}
}

private struct ComingSoon: View {
    let close: () -> Void

    var body: some View {
        VStack(spacing: PawMetrics.Spacing.l) {
            Spacer(minLength: 0)
            Text("AutoFill is coming")
                .font(PawFont.title)
                .foregroundStyle(PawColor.label)
                .multilineTextAlignment(.center)
                .accessibilityAddTraits(.isHeader)
            Text("paw can't fill passwords yet. Open paw to copy one.")
                .font(PawFont.body)
                .foregroundStyle(PawColor.labelSecondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            Button("Close", action: close)
                .buttonStyle(.pawSecondary)
        }
        .padding(PawMetrics.Spacing.xl)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(PawColor.bg.ignoresSafeArea())
    }
}
