import SwiftUI

/// A theme with every color worked out, in the shape the settings crate resolves one.
public struct Theme: Identifiable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let scheme: ColorScheme
    public let palette: Palette

    public init(id: String, name: String, scheme: ColorScheme, palette: Palette) {
        self.id = id
        self.name = name
        self.scheme = scheme
        self.palette = palette
    }

    /// Nil when a color is missing or unreadable, so a broken theme can't half apply.
    public init?(id: String, name: String, scheme: ColorScheme, colors: [String: String]) {
        guard let palette = Palette(colors: colors) else { return nil }
        self.init(id: id, name: name, scheme: scheme, palette: palette)
    }

    public static let tectrDark = Theme(id: "tectr-dark", name: "Tectr Dark", scheme: .dark, palette: .tectrDark)
    public static let tectrLight = Theme(id: "tectr-light", name: "Tectr Light", scheme: .light, palette: .tectrLight)
}

/// The mode setting: follow the system, or force one.
public enum AppearanceMode: String, CaseIterable, Identifiable, Sendable {
    case system, light, dark

    public var id: Self { self }

    public var label: String {
        switch self {
        case .system: "Follow the system"
        case .light: "Light"
        case .dark: "Dark"
        }
    }

    var forced: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }
}

/// A color by role, looked up in the palette of the current theme when it's drawn.
public struct ThemeColor: ShapeStyle {
    let role: ColorRole

    init(_ role: ColorRole) {
        self.role = role
    }

    public func resolve(in environment: EnvironmentValues) -> Color {
        environment.palette[role]
    }
}

public extension EnvironmentValues {
    @Entry var palette: Palette = .tectrDark
}

public extension View {
    /// The theme and the tint, applied once at the root. Fonts and text colors are left to the
    /// system, so forms, settings and sheets stay stock. Our own content sets them itself.
    func tectrTheme(mode: AppearanceMode = .system, dark: Theme = .tectrDark, light: Theme = .tectrLight) -> some View {
        modifier(ThemeRoot(mode: mode, dark: dark, light: light))
    }
}

private struct ThemeRoot: ViewModifier {
    let mode: AppearanceMode
    let dark: Theme
    let light: Theme
    @Environment(\.colorScheme) private var system

    func body(content: Content) -> some View {
        let scheme = mode.forced ?? system
        content
            .tint(ThemeColor.accentText)
            .environment(\.palette, scheme == .dark ? dark.palette : light.palette)
            .preferredColorScheme(mode.forced)
    }
}

extension Color {
    init(rgba: UInt32) {
        let channel = { (shift: UInt32) in Double((rgba >> shift) & 0xff) / 255 }
        self.init(.sRGB, red: channel(24), green: channel(16), blue: channel(8), opacity: channel(0))
    }

    /// `#rrggbb` or `#rrggbbaa`, the format themes are written in.
    init?(hex: String?) {
        guard let hex, hex.hasPrefix("#") else { return nil }
        let digits = hex.dropFirst()
        guard digits.count == 6 || digits.count == 8, let value = UInt32(digits, radix: 16) else { return nil }
        self.init(rgba: digits.count == 6 ? value << 8 | 0xff : value)
    }
}
