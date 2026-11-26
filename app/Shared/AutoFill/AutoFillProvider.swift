import AuthenticationServices

/// Where a request's one answer goes: the extension context, or the debug
/// harness's result line.
@MainActor
protocol AutoFillAnswers: AnyObject {
    func complete(with credential: AutoFillCredential)
    func cancel(_ reason: ASExtensionError.Code)
}

/// The provider's requests, as iOS makes them, up to their answer: what
/// was asked, the model that answers it, and the one answer each request
/// gets. The extension's controller and the app's debug harness both run
/// requests through this; only `answers` differs.
@MainActor
final class AutoFillProvider {
    /// The request under way; `nil` once it has ended.
    private(set) var model: AutoFillModel?
    weak var answers: AutoFillAnswers?
    private let open: @Sendable () -> AutoFillOpening

    init(open: @escaping @Sendable () -> AutoFillOpening) {
        self.open = open
    }

    /// The key icon (or "Passwords…") in a field of this kind: a list for
    /// these sites.
    func list(_ services: [ASCredentialServiceIdentifier], kind: AutoFillKind) -> AutoFillModel {
        start(.list(services: services.map(\.identifier)), kind: kind)
    }

    /// A suggestion above the keyboard, after `userInteractionRequired`:
    /// Face ID or the key sheet for that one entry. `nil`, with the
    /// request ended, when the suggestion is not one of paw's.
    func entry(_ request: ASCredentialRequest) -> AutoFillModel? {
        guard let asked = Self.entry(of: request) else {
            model = nil
            answers?.cancel(.credentialIdentityNotFound)
            return nil
        }
        return start(asked.request, kind: asked.kind)
    }

    /// What a suggestion answers at once (`provideCredentialWithoutUserInteraction`):
    /// always a screen. Every copy is sealed behind Face ID, which needs
    /// one, so a fill without one is never possible. Answered without
    /// opening the store, which could wait on paw's lock: a missing entry is
    /// found out on the screen that follows.
    static func answerWithoutScreen(_ request: ASCredentialRequest) -> ASExtensionError.Code {
        Self.entry(of: request) == nil ? .credentialIdentityNotFound : .userInteractionRequired
    }

    /// The entry a suggestion is for, the site it was published for (which
    /// the entry's own `url:` line is checked against), and what it fills.
    static func entry(of request: ASCredentialRequest) -> (request: AutoFillRequest, kind: AutoFillKind)? {
        guard let kind = AutoFillKind(request) else { return nil }
        let identity = request.credentialIdentity
        guard let path = identity.recordIdentifier, !path.isEmpty else { return nil }
        let service = identity.serviceIdentifier.identifier
        return (.entry(path: path, service: service.isEmpty ? nil : service), kind)
    }

    private func start(_ request: AutoFillRequest, kind: AutoFillKind) -> AutoFillModel {
        let model = AutoFillModel(request: request, kind: kind, provider: self, open: open)
        self.model = model
        return model
    }

    /// The provider's view went away: swiped down, or closed by iOS.
    func viewWentAway() {
        model?.viewWentAway()
    }

    /// The provider itself is going away: the request ends unanswered.
    func shutDown() {
        let model = model
        self.model = nil
        model?.shutDown()
    }

    /// `model`'s fill, the request's one answer if it is still under way.
    func fill(_ credential: AutoFillCredential, for model: AutoFillModel) {
        guard finish(model) else { return }
        answers?.complete(with: credential)
    }

    /// `model`'s request ends without a fill, likewise.
    func end(_ reason: ASExtensionError.Code, for model: AutoFillModel) {
        guard finish(model) else { return }
        answers?.cancel(reason)
    }

    /// Ends `model`'s request if it is the one under way, and says whether
    /// it was.
    private func finish(_ model: AutoFillModel) -> Bool {
        guard model === self.model else { return false }
        self.model = nil
        model.shutDown()
        return true
    }
}
