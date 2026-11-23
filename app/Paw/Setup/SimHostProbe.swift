import SwiftUI

#if targetEnvironment(simulator)
/// `-pawSimHostProbe 1`: starts the core's simulated git host inside the
/// app and clones from it with the simulated key, then says what happened.
/// Temporary: it answers whether `SimRemote` works in the iOS simulator.
struct SimHostProbeView: View {
    @State private var result = "Running"

    var body: some View {
        Text(result)
            .accessibilityIdentifier("probe-result")
            .padding()
            .task {
                let outcome = await Task.detached { Self.run() }.value
                result = outcome
            }
    }

    private final class Listener: ProgressListener, @unchecked Sendable {
        func progress(received: UInt32, total: UInt32) -> Bool { true }
    }

    nonisolated private static func run() -> String {
        var steps: [String] = []
        do {
            let key = SimulatedKeyHolder.shared.key
            let remote = try SimRemote.start(key: key)
            steps.append("started \(remote.address())")
            let dir = FileManager.default.temporaryDirectory
                .appendingPathComponent("paw-probe-\(UUID().uuidString)", isDirectory: true)
            let setup = try Setup.open(stateDir: dir.path)
            let connection = try KeyConnection.openForSession()
            _ = try setup.readKey(connection: connection)
            steps.append("read key")
            var state = try setup.setAddress(address: remote.address())
            state = try setup.checkHost()
            if case .confirmHost(_, _, let fingerprint) = state {
                steps.append("confirm \(fingerprint == remote.hostKeyFingerprint())")
                state = try setup.confirmHost(fingerprint: fingerprint)
            }
            steps.append("at \(state)")
            state = try setup.download(connection: connection, pin: "123456", listener: Listener())
            steps.append("downloaded: \(state)")
            state = try setup.testDecrypt(connection: connection, pin: "123456")
            steps.append("checked: \(state) logins \(remote.logins())")
            return "PROBE OK " + steps.joined(separator: " | ")
        } catch {
            return "PROBE FAILED \(error) after " + steps.joined(separator: " | ")
        }
    }
}
#endif
