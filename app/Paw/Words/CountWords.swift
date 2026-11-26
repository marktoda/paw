import Foundation

/// "1 change", "3 changes".
func changeCount(_ count: Int) -> String {
    count == 1 ? "1 change" : "\(count) changes"
}

/// What removing the store from this iPhone deletes, and the changes not
/// sent yet that it loses.
func removeStoreWarning(unpushed: Int) -> String {
    var text = Device.words("Deletes the store, its Face ID copies and your key's record from this iPhone. ")
        + "The store on the server is not touched."
    if unpushed > 0 {
        text += " \(changeCount(unpushed)) not sent yet will be lost."
    }
    return text
}

/// "1 password", "3 passwords".
func passwordCount(_ count: Int) -> String {
    count == 1 ? "1 password" : "\(count) passwords"
}

/// The search field's prompt on Home: "Search 3 passwords".
func searchPrompt(entryCount: Int) -> String {
    "Search \(passwordCount(entryCount))"
}
