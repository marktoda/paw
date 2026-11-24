import AuthenticationServices
import SwiftUI

/// The AutoFill extension (design.md 2.10). It reads the store paw set up
/// and fills a password by its Face ID copy or with the key. It never
/// writes the store, opens no network connection, never shows a password
/// and never uses the pasteboard. The screens and their model live in
/// `app/Shared/AutoFill`, where the app's debug harness drives them too.
final class CredentialProviderViewController: ASCredentialProviderViewController, AutoFillCompletion {
    private var model: AutoFillModel?
    private var host: UIHostingController<AutoFillRootView>?

    /// The key icon, or "Passwords…" in the bar: a list for these sites.
    override func prepareCredentialList(for serviceIdentifiers: [ASCredentialServiceIdentifier]) {
        show(.list(services: serviceIdentifiers.map(\.identifier)))
    }

    /// A suggestion above the keyboard. Every copy is sealed behind Face
    /// ID, which needs a screen, so this never fills by itself and answers
    /// at once, without opening the store (which could wait on paw's lock).
    /// A missing entry is found out on the screen that follows.
    override func provideCredentialWithoutUserInteraction(for credentialRequest: ASCredentialRequest) {
        end(Self.path(of: credentialRequest) == nil ? .credentialIdentityNotFound : StoreForAutoFill.answerWithoutScreen)
    }

    /// After `userInteractionRequired`: Face ID or the key sheet for that
    /// one entry.
    override func prepareInterfaceToProvideCredential(for credentialRequest: ASCredentialRequest) {
        guard let path = Self.path(of: credentialRequest) else {
            end(.credentialIdentityNotFound)
            return
        }
        show(.entry(path: path))
    }

    /// Every way the provider can go away ends its key tasks and drops the
    /// PIN: `fill` and `end` below (after `completeRequest` and
    /// `cancelRequest`), a swipe down or iOS closing the sheet (here), and
    /// the controller going away (`deinit`).
    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        model?.viewWentAway()
    }

    deinit {
        let model = model
        if Thread.isMainThread {
            MainActor.assumeIsolated { model?.shutDown() }
        } else {
            DispatchQueue.main.async { model?.shutDown() }
        }
    }

    private static func path(of request: ASCredentialRequest) -> String? {
        guard let identity = request.credentialIdentity as? ASPasswordCredentialIdentity,
              let path = identity.recordIdentifier, !path.isEmpty else { return nil }
        return path
    }

    private func show(_ request: AutoFillRequest) {
        let model = AutoFillModel(request: request, completion: self, open: { StoreForAutoFill.open() })
        self.model = model
        let host = UIHostingController(rootView: AutoFillRootView(model: model))
        self.host?.willMove(toParent: nil)
        self.host?.view.removeFromSuperview()
        self.host?.removeFromParent()
        addChild(host)
        host.view.frame = view.bounds
        host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(host.view)
        host.didMove(toParent: self)
        self.host = host
    }

    // MARK: AutoFillCompletion

    func fill(user: String, password: String) {
        let credential = ASPasswordCredential(user: user, password: password)
        extensionContext.completeRequest(withSelectedCredential: credential, completionHandler: nil)
        model?.shutDown()
        model = nil
    }

    func end(_ reason: AutoFillEnd) {
        let code: ASExtensionError.Code
        switch reason {
        case .userCanceled: code = .userCanceled
        case .userInteractionRequired: code = .userInteractionRequired
        case .credentialIdentityNotFound: code = .credentialIdentityNotFound
        case .failed: code = .failed
        }
        extensionContext.cancelRequest(withError: NSError(domain: ASExtensionErrorDomain, code: code.rawValue))
        model?.shutDown()
        model = nil
    }
}
