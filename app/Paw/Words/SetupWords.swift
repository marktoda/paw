import Foundation

/// How setup words what stopped it: what happened, what is safe, what to
/// do (design.md 2.4, S3 and S4). The core decides what went wrong; this
/// only words it.
enum SetupWords {
    /// What the host check or download found wrong with the remote.
    static func remoteWords(_ failure: RemoteFailure, during moment: SetupModel.Moment) -> SetupProblem {
        let downloading = moment == .downloading
        switch failure {
        case .noConnection(let host, let detail, let fallback):
            var words = unreachable(host: host, detail: detail, downloading: downloading)
            words.offer = fallback.map(portFallbackOffer)
            return words
        case .unreachable(let host, let detail):
            return unreachable(host: host, detail: detail, downloading: downloading)
        case .hostKeyChanged(let host, let expected, let presented, let bundled):
            return hostKeyChanged(host: host, expected: expected, presented: presented, bundled: bundled)
        case .hostKeyNotConfirmed(let host):
            return SetupProblem(
                problem: "That isn't the fingerprint \(host) showed", nextStep: "Go back and connect again.")
        case .keyNotAccepted(let host, _):
            return SetupProblem(
                problem: "\(host) doesn't know this key yet", nextStep: "Add its SSH key to your account there.")
        case .repositoryMissing(let host, let repository, let said, _):
            let repository = repository.isEmpty ? "that repository" : repository
            let host = host.isEmpty ? "The server" : host
            return SetupProblem(
                problem: "No store at that address",
                nextStep: "\(host) says \(repository) doesn't exist, or this key can't see it. Check the address "
                    + "and your account's access.",
                details: said.map { ["\(host) said: \($0)"] } ?? [],
                action: .retry, actionTitle: "Try again")
        case .refusedContent(let paths):
            return SetupProblem(
                problem: SyncFailure.refusedRemoteProblem,
                nextStep: "Rename or remove them on a computer, then try again.",
                details: paths.map { "\($0.path): \(refusedWords($0))" },
                action: .retry, actionTitle: "Try again")
        case .hostKeyBundled(let host):
            return didNotWork(hostKeyBundled(host), downloading: downloading)
        default:
            return didNotWork(remoteDetail(failure), downloading: downloading)
        }
    }

    private static func unreachable(host: String, detail: String, downloading: Bool) -> SetupProblem {
        if downloading {
            return SetupProblem(
                problem: "The download was interrupted",
                nextStep: "Nothing was kept. Check your connection and try again.",
                details: [detail], action: .retry, actionTitle: "Try again")
        }
        return SetupProblem(
            problem: "Can't reach \(host)", nextStep: "Check your connection and try again.",
            details: [detail], action: .retry, actionTitle: "Try again")
    }

    /// A step that failed for a reason the person can only retry: `detail`
    /// says which.
    private static func didNotWork(_ detail: String, downloading: Bool) -> SetupProblem {
        if downloading {
            return SetupProblem(
                problem: "The download didn't finish",
                nextStep: "Nothing was kept. Try again, or check the address and your account's access.",
                details: [detail], action: .retry, actionTitle: "Try again")
        }
        return SetupProblem(
            problem: "That didn't work", nextStep: "Try again.", details: [detail],
            action: .retry, actionTitle: "Try again")
    }

    /// A pinned host presented another key. For a host paw has no keys of
    /// its own for, the person may say they expected it.
    static func hostKeyChanged(host: String, expected: [String], presented: String,
                                       bundled: Bool) -> SetupProblem {
        let warning = "This can mean someone is intercepting the connection. Nothing was sent."
        let advice: String
        if bundled {
            advice = "\(host)'s real keys are built into paw, so this isn't \(host). Try another network."
        } else {
            advice = "If the server's admin confirms the change, tap I expected this change."
        }
        var words = SetupProblem(
            problem: "\(host)'s identity changed",
            nextStep: "\(warning) \(advice)",
            details: expected.map { "Expected \($0)" } + ["It presented \(presented)"],
            action: .startOver, actionTitle: "Start setup again")
        if !bundled {
            words.offer = SetupProblem.Offer(
                title: "I expected this change", identifier: "host-key-expected", action: .forgetHostKey)
        }
        return words
    }

    /// The failure table of design.md 2.4, S3 and S4.
    static func words(_ error: SetupError, during moment: SetupModel.Moment) -> SetupProblem {
        let downloading = moment == .downloading
        switch error {
        case .Remote(let failure):
            return remoteWords(failure, during: moment)
        case .EmptyRepository:
            return SetupProblem(
                problem: "That store is empty",
                nextStep: "Add a password to it on a computer, then try again.",
                action: .retry, actionTitle: "Try again")
        case .NotAPassStore:
            return SetupProblem(
                problem: "That isn't a pass store",
                nextStep: "Nothing there says which key it is encrypted for. Check the address.",
                details: ["No .gpg-id file at the top of the repository"])
        case .KeyCannotOpenStore:
            return SetupProblem(
                problem: "This key can't open this store",
                nextStep: "Its passwords are encrypted for another key. Plug that key in and try again.",
                action: .retry, actionTitle: "Try again")
        case .UnusableKey(let reason):
            return unusableKey(reason)
        case .WrongStep(let message):
            return SetupProblem(
                problem: "Setup got out of step", nextStep: "Go back one step and try again.",
                details: [message])
        case .Failed(let message):
            return didNotWork(message, downloading: downloading)
        case .StoreBusy:
            return didNotWork(storeBusyText, downloading: downloading)
        case .BadAddress(let problem):
            return SetupProblem(problem: "Check the address", nextStep: addressProblemWords(problem))
        }
    }

    /// Why the key can't set up a store. A missing decryption or SSH key
    /// reads as the key sheet says it.
    static func unusableKey(_ reason: UnusableKeyReason) -> SetupProblem {
        let why: String
        switch reason {
        case .noDecryptionKey, .noAuthenticationKey:
            let words = KeyErrorWords(
                reason == .noDecryptionKey ? .NoDecryptionKey : .NoAuthenticationKey, entry: "your store", purpose: .check)
            return SetupProblem(problem: words.problem, nextStep: words.nextStep)
        case .authenticationKeyNotEd25519(let algorithm):
            let words = KeyErrorWords(
                .AuthenticationKeyNotEd25519(algorithm: algorithm), entry: "your store", purpose: .check)
            return SetupProblem(problem: words.problem, nextStep: words.nextStep)
        case .pinKdfUnsupported(let detail):
            return SetupProblem(
                problem: "paw doesn't support this key's PIN setting yet",
                nextStep: "It has KDF turned on. No PIN try was used.",
                details: detail.map { [$0] } ?? [])
        case .notTheKeyRead:
            why = "This is not the key read at the start of setup; plug that one in."
        case .cardKeyMismatch:
            why = "The key read from the card does not match its fingerprint."
        case .sshKeyUnreadable(let detail):
            why = asSentence(detail)
        }
        return SetupProblem(problem: "This key can't be used", nextStep: why)
    }

    /// Forgetting the server's key didn't happen: `detail` says why.
    static func hostKeyKept(_ detail: String) -> SetupProblem {
        SetupProblem(
            problem: "paw kept this server's key", nextStep: asSentence(detail),
            action: .startOver, actionTitle: "Start setup again")
    }

    /// The host's usual port could not be reached: offers the other one.
    static func portFallbackOffer(_ fallback: PortFallback) -> SetupProblem.Offer {
        SetupProblem.Offer(
            text: "Port \(fallback.fromPort) seems blocked. Try \(fallback.host) on port \(fallback.port)?",
            title: "Try port \(fallback.port)", identifier: "use-port-fallback", action: .usePortFallback)
    }
}

