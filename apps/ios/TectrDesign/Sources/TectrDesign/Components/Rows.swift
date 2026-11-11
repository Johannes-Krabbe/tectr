import SwiftUI

/// The row that heads a device's workspaces, with room for its buttons at the end.
public struct DeviceHeader<Actions: View>: View {
    let name: String
    let online: Bool
    /// Why it isn't plainly online, such as "Offline, 2h ago".
    let meta: String?
    let actions: Actions

    public init(name: String, online: Bool, meta: String? = nil, @ViewBuilder actions: () -> Actions) {
        self.name = name
        self.online = online
        self.meta = meta
        self.actions = actions()
    }

    public var body: some View {
        HStack(spacing: Tokens.Space.sm) {
            OnlineDot(online: online)
            Text(name)
                .font(Tokens.Text.label)
                .foregroundStyle(online ? .textPrimary : .textTertiary)
            if let meta {
                Text(meta)
                    .font(Tokens.Text.meta)
                    .foregroundStyle(online ? .statusWaiting : .textTertiary)
            }
            Spacer(minLength: Tokens.Space.sm)
            actions
                .labelStyle(.iconOnly)
                .foregroundStyle(.textTertiary)
                .buttonStyle(.borderless)
        }
        .lineLimit(1)
    }
}

public extension DeviceHeader where Actions == EmptyView {
    init(name: String, online: Bool, meta: String? = nil) {
        self.init(name: name, online: online, meta: meta) { EmptyView() }
    }
}

public struct OnlineDot: View {
    let online: Bool
    @ScaledMetric(relativeTo: .footnote) private var size: CGFloat = 7

    public init(online: Bool) {
        self.online = online
    }

    public var body: some View {
        Circle()
            .fill(online ? ThemeColor.statusOnline : ThemeColor.surfaceBase)
            .stroke(online ? ThemeColor.statusOnline : ThemeColor.textTertiary, lineWidth: 1.2)
            .frame(width: size, height: size)
            .accessibilityLabel(online ? "Online" : "Offline")
    }
}

public struct TopicRow: View {
    let name: String
    let glyph: StatusGlyph
    let meta: String?
    /// Where it is, for lists that mix workspaces.
    let context: String?

    public init(name: String, glyph: StatusGlyph, meta: String? = nil, context: String? = nil) {
        self.name = name
        self.glyph = glyph
        self.meta = meta
        self.context = context
    }

    public var body: some View {
        HStack(spacing: Tokens.Space.md) {
            StatusIcon(glyph)
            VStack(alignment: .leading, spacing: Tokens.Space.xxs) {
                Text(name)
                    .font(Tokens.Text.label)
                    .foregroundStyle(glyph == .stale ? .textTertiary : .textPrimary)
                if let context {
                    Text(context)
                        .font(Tokens.Text.meta)
                        .foregroundStyle(glyph == .waiting ? .textSecondary : .textTertiary)
                }
            }
            .lineLimit(1)
            Spacer(minLength: Tokens.Space.sm)
            if let meta {
                Text(meta)
                    .font(Tokens.Text.meta.monospacedDigit())
                    // Tertiary text fails contrast on the waiting tint, so waiting meta is in its own color.
                    .foregroundStyle(glyph == .waiting ? .statusWaiting : .textTertiary)
                    .lineLimit(1)
            }
        }
        .frame(minHeight: Tokens.Size.row - Tokens.Space.lg)
    }
}

public extension View {
    /// Our surfaces under a system list, so content reads as the theme while keeping native rows.
    /// Forms, settings and sheets don't use it: they stay stock.
    func tectrList() -> some View {
        scrollContentBackground(.hidden)
            .tectrBackground()
            .foregroundStyle(.textPrimary)
    }

    /// The theme's base surface behind a screen, reaching behind the keyboard too. A plain
    /// `.background(.surfaceBase)` stops at the keyboard, which then shows the window's black.
    func tectrBackground() -> some View {
        background {
            Rectangle().fill(.surfaceBase).ignoresSafeArea()
        }
    }

    /// The panel surface for the rows of a list section, tinted when the row is waiting.
    func tectrRow(_ glyph: StatusGlyph? = nil) -> some View {
        listRowBackground(RowBackground(waiting: glyph == .waiting))
    }

    /// A row of the device tree, flat on the base surface like the desktop's sidebar.
    /// `depth` indents topics under their workspace.
    func sidebarRow(_ glyph: StatusGlyph? = nil, depth: Int = 0) -> some View {
        // Clear rather than nil: a plain list would otherwise draw the system's background.
        listRowBackground(Rectangle().fill(glyph == .waiting ? ThemeColor.statusWaitingTint : ThemeColor.surfaceBase))
            .listRowSeparator(.hidden)
            .listSectionSeparator(.hidden)
            .listRowInsets(.init(top: Tokens.Space.xs, leading: Tokens.Space.lg + CGFloat(depth) * Tokens.Space.lg,
                                 bottom: Tokens.Space.xs, trailing: Tokens.Space.lg))
    }
}

private struct RowBackground: View {
    let waiting: Bool

    var body: some View {
        Rectangle()
            .fill(.surfacePanel)
            .overlay {
                if waiting { Rectangle().fill(.statusWaitingTint) }
            }
    }
}

#Preview {
    List {
        Section {
            TopicRow(name: "Client reconnect backoff", glyph: .waiting, meta: "Waiting").tectrRow(.waiting)
            TopicRow(name: "Spawn sessions via ACP", glyph: .working, meta: "18:04").tectrRow()
            TopicRow(name: "Incremental markdown parsing", glyph: .failed, meta: "6m").tectrRow()
            TopicRow(name: "Virtualized transcript", glyph: .done, meta: "1h").tectrRow()
        } header: {
            DeviceHeader(name: "Laptop", online: true)
        }
        Section {
            TopicRow(name: "APNs token refresh", glyph: .stale, meta: "2h").tectrRow()
        } header: {
            DeviceHeader(name: "VPS", online: false, meta: "Offline, 2h ago")
        }
    }
    .tectrList()
    .tectrTheme()
}
