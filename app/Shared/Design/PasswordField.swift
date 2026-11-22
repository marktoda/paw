import SwiftUI

/// How a password character is drawn and spoken.
enum PasswordCharacter {
    case letter, digit, symbol

    init(_ character: Character) {
        if character.isLetter {
            self = .letter
        } else if character.isNumber {
            self = .digit
        } else {
            self = .symbol
        }
    }

    var color: Color {
        switch self {
        case .letter: PawColor.label
        case .digit: PawColor.passwordDigit
        case .symbol: PawColor.passwordSymbol
        }
    }

    /// A password with digits and symbols coloured. Every character may end
    /// a line, so a long password wraps by character and is never truncated.
    static func colored(_ password: String) -> AttributedString {
        var text = AttributedString()
        for character in password {
            var part = AttributedString(String(character) + "\u{200B}")
            part.foregroundColor = PasswordCharacter(character).color
            text.append(part)
        }
        return text
    }

    /// What VoiceOver says for one cell of the large-type view.
    static func spoken(_ character: Character) -> String {
        if character.isLetter {
            return character.isUppercase ? "Capital \(character)" : "Lowercase \(character)"
        }
        return symbolNames[character] ?? String(character)
    }

    private static let symbolNames: [Character: String] = [
        " ": "Space", "-": "Hyphen", "_": "Underscore", ".": "Full stop", ",": "Comma",
        "!": "Exclamation mark", "?": "Question mark", "#": "Hash", "&": "Ampersand",
        "@": "At sign", "$": "Dollar sign", "%": "Per cent", "*": "Asterisk", "+": "Plus",
        "=": "Equals", "/": "Slash", "\\": "Backslash", ":": "Colon", ";": "Semicolon",
        "'": "Apostrophe", "\"": "Quotation mark", "^": "Caret", "~": "Tilde", "|": "Vertical bar",
        "(": "Left parenthesis", ")": "Right parenthesis", "[": "Left bracket", "]": "Right bracket",
        "{": "Left brace", "}": "Right brace", "<": "Less than", ">": "Greater than", "`": "Backtick",
    ]
}

/// A 44pt glyph button for use inside a row that holds several controls.
struct PawIconButton: View {
    let symbol: String
    let label: String
    let identifier: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(PawFont.body)
                .foregroundStyle(PawColor.label)
                .frame(width: PawMetrics.tapMin, height: PawMetrics.tapMin)
                .contentShape(Rectangle())
        }
        .buttonStyle(.borderless)
        .accessibilityLabel(label)
        .accessibilityIdentifier(identifier)
    }
}

/// A label over a value, with a copy button: username, website and the like.
struct FieldRow: View {
    let label: String
    let value: String
    var onCopy: () -> Void = {}

    var body: some View {
        HStack(spacing: PawMetrics.Spacing.xs) {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                Text(label)
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                Text(value)
                    .font(PawFont.body)
                    .foregroundStyle(PawColor.label)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(label)
            .accessibilityValue(value)
            .accessibilityIdentifier("field-\(label)")
            PawIconButton(
                symbol: PawSymbol.copy, label: "Copy \(label.lowercased())",
                identifier: "copy-\(label)", action: onCopy)
        }
        .padding(.vertical, PawMetrics.Spacing.s)
        .frame(minHeight: PawMetrics.rowField)
    }
}

/// The password: always 16 dots while hidden, coloured SF Mono when shown.
struct PasswordField: View {
    var label = "Password"
    let password: String
    @Binding var isRevealed: Bool
    var onCopy: () -> Void = {}
    var onLargeType: () -> Void = {}

    var body: some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
                Text(label)
                    .font(PawFont.footnote)
                    .foregroundStyle(PawColor.labelSecondary)
                if isRevealed {
                    Text(PasswordCharacter.colored(password))
                        .font(PawFont.password)
                        .fixedSize(horizontal: false, vertical: true)
                } else {
                    Text(String(repeating: "\u{2022}", count: 16))
                        .font(PawFont.password)
                        .foregroundStyle(PawColor.label)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(label)
            .accessibilityValue(isRevealed ? password : "hidden")
            .accessibilityIdentifier("password-value")

            if isRevealed {
                PawIconButton(
                    symbol: PawSymbol.largeType, label: "Show in large type",
                    identifier: "password-large-type", action: onLargeType)
            }
            PawIconButton(
                symbol: isRevealed ? PawSymbol.hide : PawSymbol.reveal,
                label: isRevealed ? "Hide password" : "Show password",
                identifier: "password-reveal") {
                    isRevealed.toggle()
                }
            PawIconButton(
                symbol: PawSymbol.copy, label: "Copy password",
                identifier: "password-copy", action: onCopy)
        }
        .padding(.vertical, PawMetrics.Spacing.s)
        .frame(minHeight: PawMetrics.rowField)
    }
}

/// One character per numbered cell, for reading a password aloud or typing
/// it on another device.
struct LargeTypeView: View {
    let password: String

    var body: some View {
        let characters = Array(password)
        ScrollView {
            LazyVGrid(
                columns: [GridItem(.adaptive(minimum: 48), spacing: PawMetrics.Spacing.s)],
                spacing: PawMetrics.Spacing.s
            ) {
                ForEach(Array(characters.enumerated()), id: \.offset) { index, character in
                    VStack(spacing: PawMetrics.Spacing.xxs) {
                        Text(String(character))
                            .font(PawFont.passwordLarge)
                            .foregroundStyle(PasswordCharacter(character).color)
                        Text(index + 1, format: .number)
                            .font(PawFont.caption.monospacedDigit())
                            .foregroundStyle(PawColor.labelSecondary)
                    }
                    .padding(.vertical, PawMetrics.Spacing.s)
                    .frame(maxWidth: .infinity)
                    .background(
                        PawColor.fill,
                        in: RoundedRectangle(cornerRadius: PawMetrics.inlineRadius, style: .continuous))
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(
                        "\(PasswordCharacter.spoken(character)), \(index + 1) of \(characters.count)")
                    .accessibilityIdentifier("large-type-cell-\(index + 1)")
                }
            }
            .padding(PawMetrics.Spacing.l)
        }
    }
}
