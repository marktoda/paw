import SwiftUI

/// The sync status as a 30pt capsule: a glyph and a few words, never a bare dot.
struct SyncPill: View {
    enum Status {
        case synced, syncing, notPushed, offline, failed
        /// The next tap asks for the key: the gold disc, not a glyph.
        case needsKey
    }

    let status: Status
    /// What to say, e.g. "Synced 2 minutes ago" or "1 change not pushed".
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
        case .notPushed: PawSymbol.notPushed
        case .offline: PawSymbol.offline
        case .failed: PawSymbol.warning
        case .needsKey: PawSymbol.synced
        }
    }

    private var glyphColor: Color {
        switch status {
        case .synced: PawColor.success
        case .failed: PawColor.warning
        // Not pushed and offline are not errors, so they take no colour.
        case .syncing, .notPushed, .offline, .needsKey: PawColor.label
        }
    }
}
