#if DEBUG
import SwiftUI

/// Launch arguments that open the component gallery instead of the app:
/// `-pawGallery 1`, optionally `-pawGalleryPage <name>` and
/// `-pawAppearance dark|light`. Debug builds only.
enum GalleryLaunch {
    static var isRequested: Bool {
        LaunchArgument.gallery.bool
    }

    static var page: GalleryPage? {
        LaunchArgument.galleryPage.string.flatMap(GalleryPage.init(rawValue:))
    }
}

enum GalleryPage: String, CaseIterable, Identifiable {
    case colours, type, tints, symbols, rows, disc, sheet, entry, status, empty, home

    var id: String { rawValue }

    var title: String {
        switch self {
        case .colours: "Colours"
        case .type: "Type"
        case .tints: "Folder tints"
        case .symbols: "Symbols"
        case .rows: "Rows"
        case .disc: "Key disc"
        case .sheet: "Key sheet"
        case .entry: "Entry and password"
        case .status: "Status"
        case .empty: "Empty states"
        case .home: "Home"
        }
    }

    @ViewBuilder var view: some View {
        switch self {
        case .colours: ColoursPage()
        case .type: TypePage()
        case .tints: TintsPage()
        case .symbols: SymbolsPage()
        case .rows: RowsPage()
        case .disc: DiscPage()
        case .sheet: KeySheetPage()
        case .entry: GalleryApp(page: self)
        case .status: StatusPage()
        case .empty: EmptyPage()
        case .home: GalleryApp(page: self)
        }
    }

    /// The app's own screens, with their own navigation stack.
    var isTheApp: Bool {
        self == .home || self == .entry
    }
}

/// Every component in every state, with invented content, and the app's
/// own Home and Entry screens on the fixture store.
struct GalleryRoot: View {
    @State private var shownApp: GalleryPage?

    var body: some View {
        Group {
            if let page = GalleryLaunch.page {
                if page.isTheApp {
                    page.view
                } else {
                    NavigationStack { page.view }
                }
            } else {
                NavigationStack {
                    List(GalleryPage.allCases) { page in
                        if page.isTheApp {
                            Button(page.title) { shownApp = page }
                                .accessibilityIdentifier("gallery-\(page.rawValue)")
                        } else {
                            NavigationLink(page.title, value: page)
                                .accessibilityIdentifier("gallery-\(page.rawValue)")
                        }
                    }
                    .navigationTitle("Gallery")
                    .navigationDestination(for: GalleryPage.self) { page in
                        page.view
                    }
                }
                .sheet(item: $shownApp) { page in
                    page.view
                }
            }
        }
        .preferredColorScheme(LaunchArgument.colorScheme)
    }
}

#if !targetEnvironment(simulator)
/// Home and Entry need the simulated key and git host.
struct GalleryApp: View {
    let page: GalleryPage

    var body: some View {
        EmptyState(
            symbol: PawSymbol.entry, title: "\(page.title) is in the simulator only",
            message: "It shows the app's own screens on the fixture store.")
    }
}
#endif

/// A scrolling gallery page on the app background.
struct GalleryScroll<Content: View>: View {
    let title: String
    private let content: Content

    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: PawMetrics.Spacing.xl) {
                content
            }
            .padding(PawMetrics.Spacing.l)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(PawColor.bg)
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
    }
}

/// A titled group of samples, on a card or straight on the background.
struct GalleryGroup<Content: View>: View {
    let title: String
    var onCard = true
    private let content: Content

    init(_ title: String, onCard: Bool = true, @ViewBuilder content: () -> Content) {
        self.title = title
        self.onCard = onCard
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: PawMetrics.Spacing.s) {
            PawSectionHeader(title)
                .padding(.horizontal, onCard ? PawMetrics.Spacing.l : 0)
            if onCard {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.m) {
                    content
                }
                .padding(PawMetrics.Spacing.l)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    PawColor.surface,
                    in: RoundedRectangle(cornerRadius: PawMetrics.cardRadius, style: .continuous))
            } else {
                VStack(alignment: .leading, spacing: PawMetrics.Spacing.m) {
                    content
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }
}

/// Invented content shared by the gallery pages.
enum GallerySample {
    struct Entry: Identifiable {
        let name: String
        let folder: String
        let access: EntryRow.Access

        var id: String { "\(folder)/\(name)" }
    }

    static let entries: [Entry] = [
        Entry(name: "GitHub", folder: "dev", access: .faceID),
        Entry(name: "Checking account", folder: "bank", access: .faceID),
        Entry(name: "Home Wi-Fi", folder: "home", access: .faceID),
        Entry(name: "Fastmail", folder: "email", access: .needsKey),
        Entry(name: "Corner bookshop", folder: "shopping", access: .needsKey),
        Entry(name: "Fastmail app passwords", folder: "email", access: .needsKey),
        Entry(name: "Fastly", folder: "dev", access: .needsKey),
        Entry(name: "Breakfast delivery club", folder: "shopping", access: .needsKey),
    ]

    static let folders: [(name: String, count: Int)] = [
        ("bank", 9), ("dev", 27), ("email", 14), ("home", 6), ("shopping", 31),
    ]

    static func row(_ entry: Entry, match: String? = nil, action: @escaping () -> Void = {}) -> EntryRow {
        EntryRow(
            name: entry.name, subtitle: entry.folder,
            symbol: EntryGlyph.symbol(for: entry.name), tint: FolderTint(folder: entry.folder),
            access: entry.access, match: match, action: action)
    }
}
#endif
