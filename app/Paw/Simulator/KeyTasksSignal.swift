import SwiftUI

#if DEBUG && targetEnvironment(simulator)
/// For the UI tests: how many key tasks have not ended, plus one while the
/// simulated key still holds a command waiting for a touch (a cancelled
/// task ends at once, but the card's command runs on until touched or timed
/// out). A test waits for "0" before checking that nothing happened.
/// Nearly invisible, 1 by 1 point; polled, because the key's state is not
/// observable.
struct KeyTasksSignal: View {
    /// The key the tasks read.
    let keys: SimulatedKeyConnector?
    /// Key tasks not yet ended, read on every tick.
    let tasks: @MainActor () -> Int

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.25)) { _ in
            let count = tasks() + (keys?.key.waitingForTouch() == true ? 1 : 0)
            Text(verbatim: "\(count)")
                .font(.caption2)
                .frame(width: 1, height: 1)
                .opacity(0.02)
                .accessibilityLabel("Key tasks running")
                .accessibilityValue("\(count)")
                .accessibilityIdentifier("sim-key-tasks")
        }
    }
}
#endif
