import Foundation

/// "12 345 678" from an ident such as "0006:12345678".
func spokenSerial(_ ident: String) -> String {
    let serial = keySerial(ident: ident)
    var groups: [String] = []
    var rest = Substring(serial)
    while rest.count > 3 {
        groups.insert(String(rest.suffix(3)), at: 0)
        rest = rest.dropLast(3)
    }
    if !rest.isEmpty {
        groups.insert(String(rest), at: 0)
    }
    return groups.joined(separator: " ")
}

/// "2C11 41DE": the last eight hex digits of a fingerprint, in two groups.
func fingerprintEnding(_ fingerprint: String) -> String {
    fingerprintTail(fingerprint: fingerprint).joined(separator: " ")
}
