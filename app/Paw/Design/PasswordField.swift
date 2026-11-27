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

    /// A password with digits and symbols coloured. A line may end after a
    /// symbol, so "tidal-Quartz-88-ferns" breaks after a hyphen rather than
    /// inside "Quartz". A run with no symbol that is wider than the line
    /// still wraps by character: a password is never truncated.
    static func colored(_ password: String) -> AttributedString {
        var text = AttributedString()
        for character in password {
            let breaks = PasswordCharacter(character) == .symbol
            var part = AttributedString(breaks ? String(character) + "\u{200B}" : String(character))
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
    let onCopy: () -> Void

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
    let label: String
    /// Prefix of the identifiers: `<prefix>-value`, `-reveal`, `-copy`, `-large-type`.
    let identifier: String
    let password: String
    @Binding var isRevealed: Bool
    let onCopy: () -> Void
    let onLargeType: () -> Void

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
            .accessibilityIdentifier("\(identifier)-value")

            if isRevealed {
                PawIconButton(
                    symbol: PawSymbol.largeType, label: "Show in large type",
                    identifier: "\(identifier)-large-type", action: onLargeType)
            }
            PawIconButton(
                symbol: isRevealed ? PawSymbol.hide : PawSymbol.reveal,
                label: isRevealed ? "Hide \(label.lowercased())" : "Show \(label.lowercased())",
                identifier: "\(identifier)-reveal") {
                    isRevealed.toggle()
                }
            PawIconButton(
                symbol: PawSymbol.copy, label: "Copy \(label.lowercased())",
                identifier: "\(identifier)-copy", action: onCopy)
        }
        .padding(.vertical, PawMetrics.Spacing.s)
        .frame(minHeight: PawMetrics.rowField)
    }
}

/// One character per numbered cell, four cells to a group, for reading a
/// password aloud or typing it on another device. Groups keep the place
/// when the eye goes back and forth.
struct LargeTypeView: View {
    let password: String

    static let groupSize = 4

    var body: some View {
        let characters = Array(password)
        let groups = stride(from: 0, to: characters.count, by: Self.groupSize).map { start in
            Array(characters[start..<min(start + Self.groupSize, characters.count)].enumerated())
                .map { (index: start + $0.offset, character: $0.element) }
        }
        ScrollView {
            LazyVGrid(
                columns: [GridItem(.adaptive(minimum: 164), spacing: PawMetrics.Spacing.l)],
                spacing: PawMetrics.Spacing.l
            ) {
                ForEach(Array(groups.enumerated()), id: \.offset) { number, group in
                    HStack(spacing: PawMetrics.Spacing.xs) {
                        ForEach(group, id: \.index) { cell in
                            Self.cell(cell.character, index: cell.index, count: characters.count)
                        }
                        // The last group keeps the others' cell width.
                        ForEach(group.count..<Self.groupSize, id: \.self) { _ in
                            Color.clear.frame(maxWidth: .infinity)
                        }
                    }
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("large-type-group-\(number + 1)")
                }
            }
            .padding(PawMetrics.Spacing.l)
        }
    }

    private static func cell(_ character: Character, index: Int, count: Int) -> some View {
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
        .accessibilityLabel("\(PasswordCharacter.spoken(character)), \(index + 1) of \(count)")
        .accessibilityIdentifier("large-type-cell-\(index + 1)")
    }
}
