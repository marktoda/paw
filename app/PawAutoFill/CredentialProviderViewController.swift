import AuthenticationServices
import SwiftUI

/// The AutoFill extension (design.md 2.10). It reads the store paw set up
/// and fills a password, or a one-time code made from the entry, by its
/// Face ID copy or with the key. It never writes the store, opens no
/// network connection, never shows a password or code and never uses the
/// pasteboard. The screens and their model live in
/// `app/Shared/AutoFill`, where the app's debug harness drives them too.
final class CredentialProviderViewController: ASCredentialProviderViewController, AutoFillAnswers {
    private var provider: AutoFillProvider?
    private var host: UIHostingController<AutoFillRootView>?

    private var requests: AutoFillProvider {
        if let provider { return provider }
        let made = AutoFillProvider(open: { StoreForAutoFill.open() })
        made.answers = self
        provider = made
        return made
    }

    override func prepareCredentialList(for serviceIdentifiers: [ASCredentialServiceIdentifier]) {
        show(requests.list(serviceIdentifiers, kind: .password))
    }

    override func prepareOneTimeCodeCredentialList(for serviceIdentifiers: [ASCredentialServiceIdentifier]) {
        show(requests.list(serviceIdentifiers, kind: .oneTimeCode))
    }

    override func provideCredentialWithoutUserInteraction(for credentialRequest: ASCredentialRequest) {
        cancel(AutoFillProvider.answerWithoutScreen(credentialRequest))
    }

    override func prepareInterfaceToProvideCredential(for credentialRequest: ASCredentialRequest) {
        if let model = requests.entry(credentialRequest) {
            show(model)
        }
    }

    /// Every way the provider can go away ends its key tasks and drops the
    /// PIN (`AutoFillModel.shutDown`): the model's own fill or end, a swipe
    /// down or iOS closing the sheet (here), and the controller going away
    /// (`deinit`).
    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        provider?.viewWentAway()
    }

    deinit {
        let provider = provider
        if Thread.isMainThread {
            MainActor.assumeIsolated { provider?.shutDown() }
        } else {
            DispatchQueue.main.async { provider?.shutDown() }
        }
    }

    private func show(_ model: AutoFillModel) {
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

    // MARK: AutoFillAnswers

    func complete(with credential: AutoFillCredential) {
        switch credential {
        case .password(let user, let password):
            extensionContext.completeRequest(
                withSelectedCredential: ASPasswordCredential(user: user, password: password), completionHandler: nil)
        case .oneTimeCode(let code):
            extensionContext.completeOneTimeCodeRequest(
                using: ASOneTimeCodeCredential(code: code), completionHandler: nil)
        }
    }

    func cancel(_ reason: ASExtensionError.Code) {
        extensionContext.cancelRequest(withError: NSError(domain: ASExtensionErrorDomain, code: reason.rawValue))
    }
}
