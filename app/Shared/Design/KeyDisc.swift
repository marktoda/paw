import SwiftUI

/// The gold disc: "the next tap needs your key", and nothing else.
/// Flat, always with its rim. Draw it only where a tap will ask for the key.
struct KeyDisc: View {
    enum Size {
        /// 14pt, trailing mark on a row.
        case marker
        /// 36pt, leading mark on a banner.
        case banner
        /// 92pt, the key sheet.
        case hero

        var diameter: CGFloat {
            switch self {
            case .marker: PawMetrics.discMarker
            case .banner: PawMetrics.discBanner
            case .hero: PawMetrics.discHero
            }
        }

        var rim: CGFloat {
            switch self {
            case .marker: 1.5
            case .banner: 2
            case .hero: 3
            }
        }
    }

    enum Phase {
        /// Still: the key is needed.
        case needed
        /// Breathing halo: the key is waiting for a touch.
        case waiting
        /// No gold: a grey ring with a lock.
        case locked
    }

    var size: Size = .marker
    var phase: Phase = .needed

    @ScaledMetric(relativeTo: .body) private var scale: CGFloat = 1

    /// How much wider than the disc the halo reaches.
    static let haloReach: CGFloat = 1.74

    var body: some View {
        let factor = size == .hero ? 1 : min(max(scale, 1), PawMetrics.maxFixedScale)
        let diameter = size.diameter * factor
        let frame = phase == .waiting ? diameter * Self.haloReach * 1.04 : diameter
        ZStack {
            if phase == .waiting {
                KeyHalo(diameter: diameter)
            }
            if phase == .locked {
                Circle()
                    .strokeBorder(PawColor.labelTertiary, lineWidth: size.rim)
                Image(systemName: PawSymbol.keyLocked)
                    .font(.system(size: diameter * 0.42, weight: .semibold))
                    .foregroundStyle(PawColor.danger)
            } else {
                Circle()
                    .fill(PawColor.keyGold)
                    .overlay {
                        Circle().strokeBorder(PawColor.keyGoldRim, lineWidth: size.rim)
                    }
                    .frame(width: diameter, height: diameter)
            }
        }
        .frame(width: frame, height: frame)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(phase == .locked ? "Your key is locked" : "Needs your key")
        .accessibilityHidden(size == .hero)
    }
}

/// Two rings that breathe while the key waits for a touch. With Reduce
/// Motion they hold still at full strength.
private struct KeyHalo: View {
    let diameter: CGFloat

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var breathing = false

    var body: some View {
        ZStack {
            ring(reach: KeyDisc.haloReach, color: PawColor.keyGoldHaloOuter, delay: 0.2)
            ring(reach: 1.4, color: PawColor.keyGoldHalo, delay: 0)
        }
        .onAppear { breathing = true }
    }

    private func ring(reach: CGFloat, color: Color, delay: Double) -> some View {
        Circle()
            .fill(color)
            .frame(width: diameter * reach, height: diameter * reach)
            .scaleEffect(reduceMotion ? 1 : (breathing ? 1.04 : 0.9))
            .opacity(reduceMotion ? 1 : (breathing ? 1 : 0.5))
            .animation(
                reduceMotion
                    ? nil
                    : Animation.easeInOut(duration: 1.6).repeatForever(autoreverses: true).delay(delay),
                value: breathing)
    }
}
