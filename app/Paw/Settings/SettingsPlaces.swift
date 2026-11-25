import Foundation

extension AppModel {
    /// The open store's state directory, for the Settings screens. `nil`
    /// for a store that can't be written.
    var settingsStateDir: String? {
        edits?.stateDir
    }
}
