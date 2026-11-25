import AuthenticationServices
import Foundation

/// What a request fills: a password, or the current one-time code (iOS 18's
/// one-time-code AutoFill). The entries, the rules and the steps are the
/// same for every kind; what differs is here and nowhere else: which
/// entries the list shows, the words, what is taken from the opened entry,
/// and what iOS is asked for and handed.
enum AutoFillKind: Equatable, Sendable, CaseIterable {
    case password
    case oneTimeCode

    /// The kind iOS asks for, from one of paw's own suggestions
    /// (`CredentialIdentities`). Passkeys: none.
    init?(_ request: ASCredentialRequest) {
        switch request.type {
        case .password where request.credentialIdentity is ASPasswordCredentialIdentity:
            self = .password
        case .oneTimeCode where request.credentialIdentity is ASOneTimeCodeCredentialIdentity:
            self = .oneTimeCode
        default:
            return nil
        }
    }

    /// Whether the list, and the suggestions above the keyboard, offer the
    /// entry at `path` without a search: for a code, only an entry known
    /// to have one (`OneTimeCodeEntries`).
    func lists(_ path: String, knownCodes: Set<String>) -> Bool {
        switch self {
        case .password: true
        case .oneTimeCode: knownCodes.contains(path)
        }
    }

    /// The suggestion paw publishes for an entry and a site.
    func identity(domain: String, user: String, path: String) -> ASCredentialIdentity {
        let service = ASCredentialServiceIdentifier(identifier: domain, type: .domain)
        switch self {
        case .password:
            return ASPasswordCredentialIdentity(serviceIdentifier: service, user: user, recordIdentifier: path)
        case .oneTimeCode:
            return ASOneTimeCodeCredentialIdentity(serviceIdentifier: service, label: user, recordIdentifier: path)
        }
    }

    // MARK: Words

    /// How a fill starts: "Fill github.com", "Fill code for github.com".
    var fillVerb: String {
        switch self {
        case .password: "Fill"
        case .oneTimeCode: "Fill code for"
        }
    }

    /// What is filled, in a question: "Fill fastmail's password?"
    var thing: String {
        switch self {
        case .password: "password"
        case .oneTimeCode: "code"
        }
    }

    /// What an entry lacks when it has nothing to fill: "github.com has no
    /// one-time code".
    var missing: String {
        switch self {
        case .password: "password"
        case .oneTimeCode: "one-time code"
        }
    }

    /// The list for a site that has none. A code is listed once its entry
    /// has been opened on this iPhone with code suggestions on; search
    /// finds the others.
    var noneForSite: String {
        switch self {
        case .password: "No password for this site."
        case .oneTimeCode: "No codes for this site. Search for one."
        }
    }

    // MARK: The fill

    /// What this kind takes from `opened`, which is still open. A code is
    /// the current one, made now; with under 3 seconds left, the next one,
    /// which is still good once the page sends it.
    func take(from opened: OpenedEntry, entry: EntryRef) -> AutoFillTaken {
        switch self {
        case .password:
            let password = opened.password() ?? ""
            let user = fillUsername(entry: opened, path: entry.path) ?? ""
            return AutoFillTaken(
                credential: password.isEmpty ? nil : .password(user: user, password: password),
                hasCode: opened.makesOneTimeCodes())
        case .oneTimeCode:
            let nowMs = Int64(Date().timeIntervalSince1970 * 1000)
            var otp = opened.otpCode(nowMs: nowMs)
            if let current = otp, current.secondsLeft < 3 {
                otp = opened.otpCode(nowMs: nowMs + Int64(current.secondsLeft) * 1000 + 1)
            }
            let code = otp?.code ?? ""
            // A code line that makes no code says why, in the core's words.
            let problem = otp == nil ? opened.otpProblem() : nil
            return AutoFillTaken(
                credential: code.isEmpty ? nil : .oneTimeCode(code),
                hasCode: !code.isEmpty,
                unusable: problem.map { (problem: "\(entry.name)'s code doesn't work", nextStep: $0) })
        }
    }
}

/// What a fill hands to iOS.
enum AutoFillCredential: Equatable, Sendable {
    case password(user: String, password: String)
    case oneTimeCode(String)
}

/// What a kind took from an opened entry.
struct AutoFillTaken {
    /// `nil` when the entry has nothing of this kind to fill.
    let credential: AutoFillCredential?
    /// Whether the entry makes one-time codes, for `OneTimeCodeEntries`.
    let hasCode: Bool
    /// The entry has what is asked for, but it can't be filled: a problem
    /// and a next step.
    var unusable: (problem: String, nextStep: String)? = nil
}
