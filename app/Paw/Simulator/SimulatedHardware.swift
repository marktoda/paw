import Foundation

#if DEBUG && targetEnvironment(simulator)
extension SimulatorControls {
    /// The simulated key and `clock`, which moves on by `-pawSimAwayMs`
    /// each time paw goes to the background. The key is out at launch if
    /// `-pawSimUnplugged` asked for it and setup left no session open.
    @MainActor
    static func simulated(clock: SimClock, keyPort: KeyPort) -> SimulatorControls {
        let held = SimulatedKeyHolder.shared
        let away = LaunchArgument.simAwayMs.int ?? 0
        return SimulatorControls(
            advanceClock: { clock.advanceMs(ms: $0) },
            plugIn: { held.key.plugIn() },
            startsUnplugged: held.startsUnplugged && keyPort.session() == nil,
            awayMs: UInt64(max(away, 0)))
    }
}
#endif
