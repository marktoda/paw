import Foundation

/// "1 match", "3 matches": a count with its noun.
func counted(_ count: Int, _ one: String, _ many: String) -> String {
    count == 1 ? "1 \(one)" : "\(count) \(many)"
}

/// Lists of names say this many, then count the rest.
let namedAtMost = 5

/// "and 3 more", for those past the first `namedAtMost` of `count`; `nil`
/// when every one is named.
func andMore(_ count: Int) -> String? {
    count > namedAtMost ? "and \(count - namedAtMost) more" : nil
}

func changeCount(_ count: Int) -> String {
    counted(count, "change", "changes")
}

/// What removing the store from this iPhone deletes, and the changes not
/// sent yet that it loses; `nil` when they could not be counted.
func removeStoreWarning(unpushed: Int?) -> String {
    var text = Device.words("Deletes the store, its Face ID copies and your key's record from this iPhone. ")
        + "The store on the server is not touched."
    if let unpushed {
        if unpushed > 0 {
            text += " \(changeCount(unpushed)) not sent yet will be lost."
        }
    } else {
        text += " Changes not sent yet may be lost."
    }
    return text
}

func passwordCount(_ count: Int) -> String {
    counted(count, "password", "passwords")
}

func searchPromptWords(entryCount: Int) -> String {
    "Search \(passwordCount(entryCount))"
}
