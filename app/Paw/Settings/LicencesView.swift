import SwiftUI

/// Settings › About › Licences: one row that opens the list.
struct LicencesRow: View {
    var body: some View {
        NavigationLink {
            LicencesView()
        } label: {
            Text("Licences")
                .foregroundStyle(PawColor.label)
        }
        .accessibilityIdentifier("settings-licences")
    }
}

/// What the app is built from and under which licences: paw's own, then
/// every component, each opening its licence texts. Read from
/// `licences.json`, which `scripts/licences.py` writes.
struct LicencesView: View {
    private let notices = LicenceNotices.bundled

    var body: some View {
        List {
            if let notices {
                Section {
                    NavigationLink {
                        LicenceTextView(
                            title: notices.paw.name, licence: notices.paw.licence, notes: [], source: nil,
                            texts: [LicenceNotices.LicenceText(title: "LICENSE", body: notices.paw.body)])
                    } label: {
                        LicenceRow(name: notices.paw.name, detail: notices.paw.licence)
                    }
                    .accessibilityIdentifier("licence-paw")
                }
                Section {
                    ForEach(notices.components) { component in
                        NavigationLink {
                            LicenceTextView(
                                title: component.name, licence: component.licence, notes: component.notes,
                                source: component.source, texts: notices.licenceTexts(of: component))
                        } label: {
                            LicenceRow(name: component.name, detail: "\(component.version) · \(component.licence)")
                        }
                        .accessibilityIdentifier("licence-\(component.name)")
                    }
                } header: {
                    PawSectionHeader("Built with")
                }
            } else {
                Text("This build has no licence list.")
                    .foregroundStyle(PawColor.labelSecondary)
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Licences")
        .navigationBarTitleDisplayMode(.inline)
    }
}

private struct LicenceRow: View {
    let name: String
    let detail: String

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.xxs) {
            Text(name)
                .font(PawFont.body)
                .foregroundStyle(PawColor.label)
            Text(detail)
                .font(PawFont.footnote)
                .foregroundStyle(PawColor.labelSecondary)
        }
        .accessibilityElement(children: .combine)
    }
}

/// One component's licence: the expression, notes, where its source is,
/// then each text, reflowed so it reads on a phone.
struct LicenceTextView: View {
    let title: String
    let licence: String
    let notes: [String]
    let source: String?
    let texts: [LicenceNotices.LicenceText]

    var body: some View {
        // Here, not in an init: the list builds this view for every row it
        // shows, and only an opened one needs its paragraphs.
        let blocks = texts.map { (title: $0.title, paragraphs: Self.paragraphs($0.body)) }
        ScrollView {
            LazyVStack(alignment: .leading, spacing: PawMetrics.Spacing.m) {
                Text(licence)
                    .font(PawFont.headline)
                    .foregroundStyle(PawColor.label)
                    .accessibilityIdentifier("licence-expression")
                ForEach(notes, id: \.self) { note in
                    Text(note)
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                }
                if let source {
                    Text(source)
                        .font(PawFont.footnote)
                        .foregroundStyle(PawColor.labelSecondary)
                        .textSelection(.enabled)
                }
                ForEach(Array(blocks.enumerated()), id: \.offset) { index, block in
                    PawSectionHeader(Self.plainTitle(block.title))
                        .padding(.top, PawMetrics.Spacing.l)
                        .accessibilityIdentifier("licence-text-\(index)")
                    ForEach(Array(block.paragraphs.enumerated()), id: \.offset) { _, paragraph in
                        Text(paragraph)
                            .font(PawFont.footnote)
                            .foregroundStyle(PawColor.label)
                            .fixedSize(horizontal: false, vertical: true)
                            .textSelection(.enabled)
                    }
                }
            }
            .padding(PawMetrics.Spacing.l)
            .frame(maxWidth: PawMetrics.readableWidth, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(PawColor.bg)
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
    }

    /// Splits a text at blank lines and joins each paragraph's hard-wrapped
    /// lines, indented ones included, so the text reflows to the screen.
    /// A line that starts a list item ("-", "1.", "(a)") or "Copyright"
    /// keeps its break.
    static func paragraphs(_ body: String) -> [String] {
        body.components(separatedBy: "\n\n").compactMap { (chunk: String) -> String? in
            var out = ""
            for raw in chunk.split(separator: "\n", omittingEmptySubsequences: false) {
                let line = raw.trimmingCharacters(in: .whitespaces)
                guard !line.isEmpty else { continue }
                let keepsBreak = line.hasPrefix("Copyright") || Self.startsListItem(line)
                if out.isEmpty {
                    out = line
                } else {
                    out += keepsBreak ? "\n" : " "
                    out += line
                }
            }
            let trimmed = out.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : trimmed
        }
    }

    /// "- ", "* ", "• ", "1. ", "2) ", "(a) ", "(iv) ".
    private static func startsListItem(_ line: String) -> Bool {
        if line.hasPrefix("- ") || line.hasPrefix("* ") || line.hasPrefix("\u{2022}") { return true }
        let head = line.prefix(6)
        if let first = head.first, first.isNumber {
            let digits = head.prefix { $0.isNumber }
            let after = head.dropFirst(digits.count)
            return after.hasPrefix(". ") || after.hasPrefix(") ")
        }
        if head.hasPrefix("("), let close = head.firstIndex(of: ")") {
            let inside = head[head.index(after: head.startIndex)..<close]
            return !inside.isEmpty && inside.allSatisfy { $0.isLetter || $0.isNumber }
                && line[line.index(after: close)...].hasPrefix(" ")
        }
        return false
    }

    /// "MIT licence" from "LICENSE-MIT.txt", "Apache 2.0 licence" from
    /// "Apache-2.0 (standard text)", "Licence" from "LICENSE" or "COPYING".
    static func plainTitle(_ file: String) -> String {
        var name = file.split(separator: "/").last.map(String.init) ?? file
        for suffix in [" (standard text)", ".txt", ".md"] where name.hasSuffix(suffix) {
            name = String(name.dropLast(suffix.count))
        }
        switch name.uppercased() {
        case "LICENSE", "LICENCE", "COPYING": return "Licence"
        case "COPYRIGHT": return "Copyright"
        case "UNLICENSE": return "The Unlicense"
        default: break
        }
        if name.uppercased().hasPrefix("LICENSE-") {
            name = String(name.dropFirst("LICENSE-".count))
        }
        let known = ["APACHE": "Apache", "ZLIB": "zlib", "UNICODE": "Unicode", "other-bits": "Other"]
        if let word = known[name] {
            name = word
        }
        if name == "Other" { return "Other terms" }
        name = name.replacingOccurrences(of: "_WITH_", with: " with ")
            .replacingOccurrences(of: "_", with: " ")
            .replacingOccurrences(of: "-", with: " ")
        return "\(name) licence"
    }
}

/// `licences.json`: paw's own licence, the components, and their texts
/// stored once each (components refer to them by index).
struct LicenceNotices: Decodable {
    struct Own: Decodable {
        let name: String
        let licence: String
        let body: String
    }

    struct Component: Decodable, Identifiable {
        let name: String
        let version: String
        let licence: String
        let source: String
        let notes: [String]
        let texts: [Int]

        var id: String { "\(name) \(version)" }
    }

    struct LicenceText: Decodable {
        let title: String
        let body: String
    }

    let paw: Own
    let components: [Component]
    let texts: [LicenceText]

    func licenceTexts(of component: Component) -> [LicenceText] {
        component.texts.compactMap { texts.indices.contains($0) ? texts[$0] : nil }
    }

    static let bundled: LicenceNotices? = {
        guard let url = Bundle.main.url(forResource: "licences", withExtension: "json"),
              let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(LicenceNotices.self, from: data)
    }()
}
