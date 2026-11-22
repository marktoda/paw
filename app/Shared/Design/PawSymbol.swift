import SwiftUI

/// SF Symbols names, one per concept. The key itself is never a symbol: it is `KeyDisc`.
enum PawSymbol {
    static let plugIn = "cable.connector"
    static let unplugged = "cable.connector.slash"
    static let keyLocked = "lock.fill"
    static let faceID = "faceid"
    static let folder = "folder.fill"
    static let entry = "ellipsis.rectangle"
    static let copy = "document.on.document"
    static let reveal = "eye"
    static let hide = "eye.slash"
    static let largeType = "textformat.size.larger"
    static let synced = "checkmark.circle.fill"
    static let syncing = "arrow.trianglehead.2.clockwise.rotate.90"
    static let notPushed = "arrow.up.circle"
    static let offline = "wifi.slash"
    static let warning = "exclamationmark.triangle.fill"
    static let error = "exclamationmark.circle.fill"
    static let search = "magnifyingglass"
    static let add = "plus"
    static let settings = "slider.horizontal.3"
    static let stepDone = "checkmark"
    static let disclosure = "chevron.right"

    /// Every symbol above with the concept it stands for, for the gallery.
    static let all: [(concept: String, name: String)] = [
        ("Plug in the key", plugIn),
        ("Key unplugged", unplugged),
        ("Key locked", keyLocked),
        ("Face ID window", faceID),
        ("Folder", folder),
        ("Entry, fallback", entry),
        ("Copy", copy),
        ("Reveal", reveal),
        ("Hide", hide),
        ("Large type", largeType),
        ("Synced", synced),
        ("Syncing", syncing),
        ("Not yet pushed", notPushed),
        ("Offline", offline),
        ("Warning", warning),
        ("Error", error),
        ("Search", search),
        ("Add", add),
        ("Settings", settings),
        ("Step done", stepDone),
        ("Disclosure", disclosure),
    ]
}

/// The glyph on an entry's chip, matched on the entry's name. Nothing is fetched.
enum EntryGlyph {
    /// Checked in order; the first keyword found in the lower-cased name wins.
    static let table: [(keyword: String, symbol: String)] = [
        ("mail", "envelope.fill"),
        ("bank", "building.columns.fill"),
        ("checking", "building.columns.fill"),
        ("savings", "building.columns.fill"),
        ("wi-fi", "wifi"),
        ("wifi", "wifi"),
        ("shop", "cart.fill"),
        ("store", "cart.fill"),
        ("delivery", "cart.fill"),
        ("git", "chevron.left.forwardslash.chevron.right"),
        ("fastly", "chevron.left.forwardslash.chevron.right"),
        ("work", "briefcase.fill"),
        ("home", "house.fill"),
        ("air", "airplane"),
        ("flight", "airplane"),
    ]

    static func symbol(for name: String) -> String {
        let lowered = name.lowercased()
        return table.first { lowered.contains($0.keyword) }?.symbol ?? PawSymbol.entry
    }
}
