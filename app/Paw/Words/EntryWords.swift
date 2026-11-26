import Foundation

/// "Clears in 45 seconds.": how long a copy stays on the pasteboard.
@MainActor var clearsIn: String {
    "Clears in \(Int(Clipboard.lifetime)) seconds."
}

/// Lock now's toast: how many passwords need the key again.
func lockedToast(ended: Int) -> String {
    "Locked. \(passwordCount(ended)) \(ended == 1 ? "needs" : "need") your key again."
}

extension ClosedEntry.Reason {
    /// What the closed entry's screen says.
    @MainActor var title: String {
        switch self {
        case .away: "Closed after \(AppModel.openEntryBackgroundLimitMs / 60_000) minutes away"
        case .deviceLocked: "Closed when your iPhone locked"
        case .copyEnded: "Its Face ID time ended"
        }
    }
}

extension Recipient {
    /// "key ID 1234 5678 9ABC DEF0", for a sentence.
    var spoken: String {
        switch self {
        case .keyId: return "key ID \(grouped)"
        case .fingerprint: return "key \(grouped)"
        case .hidden: return "a hidden recipient"
        }
    }

    /// The hex alone, in groups of four so it can be read aloud.
    var grouped: String {
        let hex: String
        switch self {
        case .keyId(let id): hex = id
        case .fingerprint(let fingerprint): hex = fingerprint
        case .hidden: return "Hidden"
        }
        return fingerprintGroups(text: hex).joined(separator: " ")
    }
}

/// Times as the screens say them: "6:40 PM".
func clockTime(ms: Int64) -> String {
    Date(timeIntervalSince1970: TimeInterval(ms) / 1000).formatted(date: .omitted, time: .shortened)
}
