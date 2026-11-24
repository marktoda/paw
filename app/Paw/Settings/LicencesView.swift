import SwiftUI

/// Settings › Licences: one row that opens the list.
struct LicencesSettingsSection: View {
    var body: some View {
        Section {
            NavigationLink {
                LicencesView()
            } label: {
                Text("Licences")
                    .foregroundStyle(PawColor.label)
            }
            .accessibilityIdentifier("settings-licences")
        }
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
                    PawSectionHeader(block.title)
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
    /// lines. Lines that start indented, with a list mark or with
    /// "Copyright" keep their break.
    static func paragraphs(_ body: String) -> [String] {
        body.components(separatedBy: "\n\n").compactMap { (chunk: String) -> String? in
            var out = ""
            for line in chunk.split(separator: "\n", omittingEmptySubsequences: false) {
                let keepsBreak = line.hasPrefix("Copyright")
                    || (line.first.map { " \t-*•(".contains($0) || $0.isNumber } ?? true)
                if out.isEmpty {
                    out = String(line)
                } else {
                    out += keepsBreak ? "\n" : " "
                    out.append(contentsOf: line)
                }
            }
            let trimmed = out.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : trimmed
        }
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
