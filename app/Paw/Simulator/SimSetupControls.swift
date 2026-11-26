import SwiftUI

#if DEBUG && targetEnvironment(simulator)
/// The simulated git host follows setup as the screens show it: it starts
/// with the setup flow, presents another key once one was trusted
/// (`changedHostKey`), and lets the next download through once one failed
/// (`dropMidClone`, `SimTaskFailed`). Applied once, to the setup flow.
struct SimSetupHostWatch: ViewModifier {
    let model: SetupModel

    func body(content: Content) -> some View {
        content
            .onAppear {
                _ = SimulatedHostHolder.shared
            }
            .onChange(of: model.screen) { old, new in
                guard case .confirmHost = old else { return }
                switch new {
                case .download, .keyNotAccepted: SimulatedHostHolder.shared.hostConfirmed()
                default: break
                }
            }
            .onChange(of: failed) { _, failed in
                if failed, case .download = model.screen {
                    SimulatedHostHolder.shared.downloadFailed()
                }
            }
    }

    private var failed: Bool {
        SimTaskFailed.shown(ownFailure: model.problem != nil, sheet: model.sheet == nil ? nil : model.keyTask)
    }
}

/// A download cancelled while the key waited for its touch leaves the key
/// waiting. The download screen then offers to time it out, as the key's
/// own timeout does on a device.
struct SimTouchLeftWaiting: ViewModifier {
    let model: SetupModel
    @State private var waiting = false

    func body(content: Content) -> some View {
        content
            .simulatorActions(waiting && model.sheet == nil ? [timeOut] : [])
            .onChange(of: model.keyTask.isShown) { _, shown in
                if !shown, model.keyTask.stage == .touch {
                    waiting = true
                }
            }
            .onChange(of: model.keyTask.stage) { _, stage in
                // A new command waits for a touch, so any cancelled one has ended.
                if stage == .touch {
                    waiting = false
                }
            }
    }

    private var timeOut: SimulatorAction {
        SimulatorAction(title: "time out the key's touch", identifier: "simulated-time-out-touch") {
            waiting = false
            model.reader.simulated?.key.timeOutTouch()
        }
    }
}
#endif
