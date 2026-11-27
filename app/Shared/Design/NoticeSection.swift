import SwiftUI

/// A problem shown inline at the top of the list it happened in.
struct Notice: Equatable {
    let problem: String
    let nextStep: String
    /// When set, the notice offers to open this entry with the key instead.
    var keyEntry: EntryRef? = nil
}

/// A Face ID or key problem from the last tap, said where it happened, with
/// Use your key when the notice offers it.
struct NoticeSection: View {
    let notice: Notice?
    let openWithKey: (EntryRef) -> Void

    var body: some View {
        if let notice {
            Section {
                ErrorRow(
                    kind: .warning, problem: notice.problem, nextStep: notice.nextStep,
                    actionTitle: notice.keyEntry == nil ? nil : "Use your key",
                    actionNeedsKey: true
                ) {
                    if let entry = notice.keyEntry {
                        openWithKey(entry)
                    }
                }
                .pawListRow()
            }
        }
    }
}
