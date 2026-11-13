import SwiftUI

/// Something an agent needs from you. On iOS it takes the message box's place, on its glass.
public struct RequestCard: View {
    public struct Option: Identifiable, Hashable, Sendable {
        public enum Role: Sendable { case primary, secondary, reject }

        public let id: String
        public let name: String
        public let role: Role

        public init(id: String, name: String, role: Role) {
            self.id = id
            self.name = name
            self.role = role
        }
    }

    let kind: String
    let position: String?
    let command: String?
    let reason: String?
    let options: [Option]
    let onSelect: (Option) -> Void

    public init(
        kind: String,
        position: String? = nil,
        command: String?,
        reason: String?,
        options: [Option],
        onSelect: @escaping (Option) -> Void
    ) {
        self.kind = kind
        self.position = position
        self.command = command
        self.reason = reason
        self.options = options
        self.onSelect = onSelect
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: Tokens.Space.lg) {
            HStack(spacing: Tokens.Space.sm) {
                StatusIcon(.waiting)
                Text(kind).font(Tokens.Text.heading)
                Spacer()
                if let position {
                    Text(position).font(Tokens.Text.meta).foregroundStyle(.textTertiary)
                }
            }
            if let command {
                CodeBlock(command)
            }
            if let reason {
                Text(reason)
                    .font(Tokens.Text.body)
                    .foregroundStyle(.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            // Side by side when the names fit, stacked when they don't.
            ViewThatFits(in: .horizontal) {
                HStack(spacing: Tokens.Space.sm) { buttons }
                VStack(spacing: Tokens.Space.sm) { buttons }
            }
        }
    }

    private var buttons: some View {
        ForEach(options) { option in
            button(option)
        }
    }

    // Bordered rather than glass, because the card is already on glass.
    @ViewBuilder
    private func button(_ option: Option) -> some View {
        let label = Text(option.name)
            .font(Tokens.Text.label)
            .lineLimit(1)
            .frame(maxWidth: .infinity, minHeight: Tokens.Size.control - Tokens.Space.lg)
        switch option.role {
        case .primary:
            Button { onSelect(option) } label: { label }
                .buttonStyle(.borderedProminent)
                .tint(ThemeColor.accentSolid)
        case .secondary:
            Button { onSelect(option) } label: { label }
                .buttonStyle(.bordered)
        case .reject:
            Button(role: .destructive) { onSelect(option) } label: { label }
                .buttonStyle(.bordered)
        }
    }
}

#Preview {
    VStack {
        Spacer()
        RequestCard(
            kind: "Run a command",
            position: "1 of 2 waiting",
            command: "rm -rf target/debug/incremental",
            reason: "Clears stale incremental artifacts that break the reconnect tests.",
            options: [
                .init(id: "allow", name: "Allow", role: .primary),
                .init(id: "always", name: "Always allow rm in this topic", role: .secondary),
                .init(id: "reject", name: "Deny", role: .reject),
            ],
            onSelect: { _ in }
        )
        .padding(Tokens.Space.lg)
        .glassEffect(.regular, in: .rect(cornerRadius: Tokens.Radius.lg))
        .padding()
    }
    .tectrTheme()
}
