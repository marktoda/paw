import AuthenticationServices
import SwiftUI

/// Spike: shows the same key test inside the AutoFill extension, to learn
/// whether an extension can reach a USB-C key at all.
final class CredentialProviderViewController: ASCredentialProviderViewController {
    override func viewDidLoad() {
        super.viewDidLoad()

        let host = UIHostingController(rootView: ExtensionRoot { [weak self] in
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

private struct ExtensionRoot: View {
    let close: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            SelfTestView(host: "AutoFill")
            Button("Close", action: close)
                .padding()
        }
    }
}
