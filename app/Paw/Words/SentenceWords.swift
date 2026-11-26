import Foundation

/// "it has no decryption key" as a sentence: "It has no decryption key."
func asSentence(_ text: String) -> String {
    let capitalised = capitalised(text)
    guard !capitalised.isEmpty else { return capitalised }
    return capitalised.hasSuffix(".") ? capitalised : capitalised + "."
}

func capitalised(_ text: String) -> String {
    text.prefix(1).uppercased() + text.dropFirst()
}
