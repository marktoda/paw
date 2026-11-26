import Foundation

/// How setup words what stopped it: what happened, what is safe, what to
/// do (design.md 2.4, S3 and S4). The core decides what went wrong; this
/// only words it.
enum SetupWords {
    /// The words for what the last host check or download found, when it
    /// has words of its own.
    static func remoteWords(_ remote: RemoteMessage) -> SetupProblem? {
        switch remote.reason {
        case .repositoryNotFound:
            let repository = remote.repository.isEmpty ? "that repository" : remote.repository
            let host = remote.host.isEmpty ? "The server" : remote.host
            return SetupProblem(
                problem: "No store at that address",
                nextStep: "\(host) says \(repository) doesn't exist, or this key can't see it. Check the address "
                    + "and your account's access.",
                details: remote.said.map { ["\(host) said: \($0)"] } ?? [],
                action: .retry, actionTitle: "Try again")
        case .emptyRepository:
            return words(.EmptyRepository, during: .downloading)
        case .notAPassStore:
            return words(.NotAPassStore, during: .downloading)
        case .keyNotAccepted, .unreachable, .hostKeyChanged, .other:
            return nil
        }
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
        case .HostUnreachable(let host, let detail):
            if downloading {
                return SetupProblem(
                    problem: "The download was interrupted",
                    nextStep: "Nothing was kept. Check your connection and try again.",
                    details: [detail], action: .retry, actionTitle: "Try again")
            }
            return SetupProblem(
                problem: "Can't reach \(host)", nextStep: "Check your connection and try again.",
                details: [detail], action: .retry, actionTitle: "Try again")
        case .HostKeyChanged(let host, let expected, let presented):
            // `show` knows whether the host's keys ship with paw; without
            // that, nothing is offered but starting again.
            return hostKeyChanged(host: host, expected: expected, presented: presented, bundled: true)
        case .HostKeyNotConfirmed(let host):
            return SetupProblem(
                problem: "That isn't the fingerprint \(host) showed", nextStep: "Go back and connect again.")
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
        case .CannotOpenEntry(let entry, let detail):
            return SetupProblem(
                problem: "Your key couldn't open \(entry)",
                nextStep: "The store is downloaded. Try again; if it keeps failing, that file may be damaged.",
                details: [detail], action: .retry, actionTitle: "Try again")
        case .UnusableKey(let reason):
            return unusableKey(reason)
        case .WrongStep(let message):
            return SetupProblem(
                problem: "Setup got out of step", nextStep: "Go back one step and try again.",
                details: [message])
        case .Failed, .StoreBusy, .HostKeyBundled:
            let message = switch error {
            case .Failed(let message): message
            case .HostKeyBundled(let host): hostKeyBundled(host)
            default: storeBusyText
            }
            if downloading {
                return SetupProblem(
                    problem: "The download didn't finish",
                    nextStep: "Nothing was kept. Try again, or check the address and your account's access.",
                    details: [message], action: .retry, actionTitle: "Try again")
            }
            return SetupProblem(
                problem: "That didn't work", nextStep: "Try again.", details: [message],
                action: .retry, actionTitle: "Try again")
        case .BadAddress(let problem):
            return SetupProblem(problem: "Check the address", nextStep: addressProblemWords(problem))
        case .KeyNotAccepted(let host, _):
            return SetupProblem(
                problem: "\(host) doesn't know this key yet", nextStep: "Add its SSH key to your account there.")
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

