import SwiftUI

public extension StatusGlyph {
    /// A topic shows its most urgent session. An unread result is something for you to do and a
    /// working agent isn't, so done comes before working.
    static let byUrgency: [StatusGlyph] = [.waiting, .failed, .done, .working, .read, .idle, .stale]

    var urgency: Int { Self.byUrgency.firstIndex(of: self) ?? Self.byUrgency.count }

    var label: String {
        switch self {
        case .working: "Working"
        case .waiting: "Waiting"
        case .failed: "Failed"
        case .done, .read: "Done"
        case .idle: "Idle"
        case .stale: "Offline"
        }
    }

    var color: ThemeColor {
        switch self {
        case .working: .statusWorking
        case .waiting: .statusWaiting
        case .failed: .statusFailed
        case .done: .statusDone
        case .read: .statusRead
        case .idle: .statusIdle
        case .stale: .statusStale
        }
    }
}

public extension EnvironmentValues {
    /// `appearance.motion.status_icons`. Reduce Motion stops them as well.
    @Entry var animatesStatusIcons = true
}

public struct StatusIcon: View {
    let glyph: StatusGlyph
    @Environment(\.animatesStatusIcons) private var animates
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @ScaledMetric(relativeTo: .subheadline) private var size = Tokens.Size.icon

    public init(_ glyph: StatusGlyph) {
        self.glyph = glyph
    }

    public var body: some View {
        Group {
            if glyph == .working && animates && !reduceMotion {
                // Angles come from the clock rather than an animation, so every arc on screen turns in step.
                TimelineView(.animation) { context in
                    let turn = context.date.timeIntervalSinceReferenceDate / Tokens.Motion.turn
                    image.rotationEffect(.degrees(turn.truncatingRemainder(dividingBy: 1) * 360))
                }
            } else {
                image
            }
        }
        .frame(width: size, height: size)
        .accessibilityLabel(glyph.label)
    }

    private var image: some View {
        glyph.image
            .resizable()
            .foregroundStyle(glyph.color)
            .contentTransition(.symbolEffect(.replace))
    }
}

#Preview {
    VStack(alignment: .leading, spacing: Tokens.Space.md) {
        ForEach(StatusGlyph.byUrgency, id: \.self) { glyph in
            Label { Text(glyph.label) } icon: { StatusIcon(glyph) }
        }
    }
    .padding()
    .tectrTheme()
}
