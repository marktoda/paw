import SwiftUI

/// The sync status as a 30pt capsule: a glyph and a few words, never a bare dot.
struct SyncPill: View {
    enum Status {
        case synced, syncing, offline, failed
        /// When paw last checked, with nothing to say about the result.
        case checked
        /// The next tap asks for the key: the gold disc, not a glyph.
        case needsKey
    }

    let status: Status
    /// What to say, e.g. "Up to date · checked 2 min ago".
    let text: String

    var body: some View {
        HStack(spacing: 6) {
            if status == .needsKey {
                KeyDisc(size: .marker)
            } else {
                Image(systemName: symbol)
                    .font(PawFont.footnote.weight(.medium))
                    .foregroundStyle(glyphColor)
            }
            Text(text)
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.label)
        }
        .padding(.horizontal, PawMetrics.Spacing.m)
        .padding(.vertical, PawMetrics.Spacing.xs)
        .frame(minHeight: PawMetrics.pill)
        .background(status == .failed ? PawColor.warningFill : PawColor.fill, in: Capsule())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(status == .needsKey ? "Sync status: \(text). Needs your key." : "Sync status: \(text)")
        .accessibilityIdentifier("sync-pill")
    }

    private var symbol: String {
        switch status {
        case .synced: PawSymbol.synced
        case .syncing: PawSymbol.syncing
        case .checked: PawSymbol.checked
        case .offline: PawSymbol.offline
        case .failed: PawSymbol.warning
        case .needsKey: PawSymbol.synced
        }
    }

    private var glyphColor: Color {
        switch status {
        case .synced: PawColor.success
        case .failed: PawColor.warning
        // Offline is not an error, so it takes no colour.
        case .syncing, .checked, .offline, .needsKey: PawColor.label
        }
    }
}
