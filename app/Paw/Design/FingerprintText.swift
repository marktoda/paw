import SwiftUI

/// A fingerprint or key ID to compare by eye: monospace, in groups of four,
/// four groups a line, with "SHA256:" on a line of its own. Never cut short
/// or shrunk: at a large text size a line wraps between groups, and the
/// view grows. A person is never asked to trust what they can't read.
struct FingerprintText: View {
    let fingerprint: String
    var font: Font = PawFont.footnote.monospaced()
    var color: Color = PawColor.labelSecondary

    private static let groupsPerLine = 4

    var body: some View {
        Text(Self.lines(fingerprint))
            .font(font)
            .foregroundStyle(color)
            .multilineTextAlignment(.leading)
            .fixedSize(horizontal: false, vertical: true)
            .textSelection(.enabled)
            .accessibilityLabel(fingerprint)
    }

    /// "SHA256:\nuNiV ztFv 8Wq2 0oYq\n…": a prefix up to a colon on its own
    /// line, then the rest in groups of four, four groups a line.
    static func lines(_ fingerprint: String) -> String {
        var prefix = ""
        var hash = Substring(fingerprint)
        if let colon = fingerprint.firstIndex(of: ":") {
            prefix = String(fingerprint[...colon]) + "\n"
            hash = fingerprint[fingerprint.index(after: colon)...]
        }
        let groups = fingerprintGroups(text: String(hash))
        let rows = stride(from: 0, to: groups.count, by: groupsPerLine).map { first in
            groups[first..<min(first + groupsPerLine, groups.count)].joined(separator: " ")
        }
        return prefix + rows.joined(separator: "\n")
    }
}
