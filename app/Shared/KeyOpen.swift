import Foundation

/// Opening an entry with the key, in the app and in AutoFill alike: the
/// core's open task, which keeps a Face ID copy for `FaceIdOpener.windowSecs`
/// where copies can be kept, worded for the entry in `table`'s words. What
/// each side does with the open entry, and with a copy an open kept after
/// its sheet let go of it, is its own.
struct KeyOpen {
    let entry: EntryRef
    let store: PassStore
    let port: KeyPort
    let copies: CopyCache?
    let table: WordTable

    /// What an open brought.
    struct Opened {
        /// Open until closed: whoever takes it closes it.
        let entry: OpenedEntry
        /// Until when it opens with Face ID; `nil` if no copy was kept.
        let faceIdUntilMs: Int64?
        /// Why the copy that was asked for wasn't kept.
        let copyFailed: PawError?
    }

    /// The open as a key job. `opened` takes the entry once the open is
    /// done; `failed` may take a key error the side shows itself, else the
    /// sheet says it. An open the sheet let go of (Cancel, Lock now) can
    /// still finish, in the last milliseconds after the card answered
    /// (`core/src/key/task.rs`): its entry is closed here, and
    /// `lateCopyKept` removes the copy it kept.
    @MainActor
    func job(
        opened: @escaping (Opened) -> KeySheetAfterEnd,
        failed: @escaping (KeyError) -> KeySheetAfterEnd = { _ in .standard },
        lateCopyKept: @escaping () -> Void
    ) -> KeyJob<Never> {
        let entry = entry, store = store, port = port, copies = copies
        let words = KeyErrorWords.Context(entry: entry.name, table: table)
        return KeyJob(
            begin: { listener in
                port.openEntry(
                    store: store, entry: entry.path, copies: copies, ttlSecs: FaceIdOpener.windowSecs,
                    listener: listener)
            },
            words: { words },
            working: { progress in
                (KeySheetWords.opening(entry.name), progress.map(\.counted))
            },
            ended: { end in
                switch end {
                case .done(.opened(let open, let until, let copyFailed)):
                    return opened(Opened(entry: open, faceIdUntilMs: until, copyFailed: copyFailed))
                case .failed(let error):
                    return failed(error)
                default:
                    return .standard
                }
            },
            letGo: { end in
                guard case .done(.opened(let open, let until, _)) = end else { return }
                open.close()
                if until != nil, copies != nil {
                    lateCopyKept()
                }
            })
    }
}
