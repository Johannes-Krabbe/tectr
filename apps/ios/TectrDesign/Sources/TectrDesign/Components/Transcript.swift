import SwiftUI

public extension EnvironmentValues {
    /// `appearance.motion.transcript`. Reduce Motion stops it as well.
    @Entry var animatesTranscript = true
}

/// One block of an agent's message. Everything is one size: headings stand out by weight and
/// a gray marker, and code by its font.
public enum MessageBlock: Hashable, Sendable {
    /// Inline markdown.
    case paragraph(String)
    case heading(String)
    case bullets([String])
    case code(language: String?, text: String)
}

public struct MessageView: View {
    let blocks: [MessageBlock]

    public init(_ blocks: [MessageBlock]) {
        self.blocks = blocks
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: Tokens.Space.md) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                switch block {
                case .paragraph(let text):
                    Text(inline(text))
                case .heading(let text):
                    Text("\(Text("# ").foregroundStyle(.textTertiary))\(Text(text).fontWeight(.semibold))")
                case .bullets(let items):
                    VStack(alignment: .leading, spacing: Tokens.Space.xs) {
                        ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                            HStack(alignment: .firstTextBaseline, spacing: Tokens.Space.sm) {
                                Circle().fill(.textTertiary).frame(width: 4, height: 4)
                                    .alignmentGuide(.firstTextBaseline) { $0[VerticalAlignment.center] + 4 }
                                Text(inline(item))
                            }
                        }
                    }
                case .code(_, let text):
                    CodeBlock(text)
                }
            }
        }
        .font(Tokens.Text.reading)
        .foregroundStyle(.textPrimary)
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Inline markdown, with code in the code font rather than the system's monospaced one.
func inline(_ markdown: String) -> AttributedString {
    var text = (try? AttributedString(
        markdown: markdown,
        options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
    )) ?? AttributedString(markdown)
    for run in text.runs where run.inlinePresentationIntent?.contains(.code) == true {
        text[run.range].font = Tokens.Text.codeBody
    }
    return text
}

public struct CodeBlock: View {
    let text: String

    public init(_ text: String) {
        self.text = text
    }

    public var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Text(text)
                .font(Tokens.Text.codeBody)
                .foregroundStyle(.syntaxPlain)
                .fixedSize()
                .padding(Tokens.Space.md)
        }
        .background(.surfaceRaised, in: .rect(cornerRadius: Tokens.Radius.sm))
    }
}

/// What you sent, set apart from the agent's messages.
public struct PromptView: View {
    let text: String
    let images: Int
    @ScaledMetric(relativeTo: .callout) private var thumbnail: CGFloat = 56

    public init(_ text: String, images: Int = 0) {
        self.text = text
        self.images = images
    }

    public var body: some View {
        VStack(alignment: .trailing, spacing: Tokens.Space.sm) {
            if images > 0 {
                HStack(spacing: Tokens.Space.xs) {
                    ForEach(0..<images, id: \.self) { _ in
                        ImagePlaceholder().frame(width: thumbnail, height: thumbnail)
                    }
                }
            }
            Text(inline(text))
                .font(Tokens.Text.reading)
                .foregroundStyle(.textPrimary)
                .textSelection(.enabled)
                .padding(.horizontal, Tokens.Space.md)
                .padding(.vertical, Tokens.Space.sm)
                .background(.surfaceRaised, in: .rect(cornerRadius: Tokens.Radius.md))
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.leading, Tokens.Space.xxl)
    }
}

/// Stands in for an image until its thumbnail arrives.
public struct ImagePlaceholder: View {
    public init() {}

    public var body: some View {
        RoundedRectangle(cornerRadius: Tokens.Radius.xs)
            .fill(.surfaceControl)
            .overlay {
                Image(systemName: "photo").foregroundStyle(.textTertiary)
            }
    }
}

/// What an agent does between its messages, as one line.
public enum ActivityState: Hashable, Sendable {
    /// The newest step still going.
    case running(String)
    case waiting
    /// Such as "Worked for 12s, 6 steps".
    case finished(String, failed: Int)
}

public struct ActivityLine: View {
    let state: ActivityState
    let expanded: Bool
    @Environment(\.animatesTranscript) private var animates
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    public init(_ state: ActivityState, expanded: Bool = false) {
        self.state = state
        self.expanded = expanded
    }

    public var body: some View {
        HStack(spacing: Tokens.Space.xs) {
            switch state {
            case .running(let step):
                if animates && !reduceMotion {
                    Sweep(text: step)
                } else {
                    Text(step).foregroundStyle(.textTertiary)
                }
            case .waiting:
                Text("Waiting for you").foregroundStyle(.statusWaiting)
            case .finished(let summary, let failed):
                Text(summary).foregroundStyle(.textTertiary)
                if failed > 0 {
                    Text("\(failed) failed").foregroundStyle(.statusFailed)
                }
            }
            if state != .waiting {
                Image(systemName: "chevron.right")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.textTertiary)
                    .rotationEffect(.degrees(expanded ? 90 : 0))
            }
        }
        .font(Tokens.Text.meta)
        .lineLimit(1)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(.rect)
    }
}

/// Tertiary text with a primary highlight passing across it.
private struct Sweep: View {
    let text: String

    var body: some View {
        Text(text)
            .foregroundStyle(.textTertiary)
            .overlay {
                TimelineView(.animation) { context in
                    let phase = context.date.timeIntervalSinceReferenceDate
                        .truncatingRemainder(dividingBy: Tokens.Motion.sweep) / Tokens.Motion.sweep
                    Text(text)
                        .foregroundStyle(.textPrimary)
                        .mask {
                            GeometryReader { proxy in
                                let width = proxy.size.width
                                LinearGradient(colors: [.clear, .white, .clear], startPoint: .leading, endPoint: .trailing)
                                    .frame(width: width * 0.4)
                                    .offset(x: -width * 0.4 + phase * width * 1.4)
                            }
                        }
                }
            }
            .contentTransition(.opacity)
            .animation(.easeInOut(duration: Tokens.Motion.fade), value: text)
    }
}

/// One step of an activity, unfolded.
public struct ToolCallRow: View {
    public enum Status: Sendable { case running, done, failed }

    let kind: String
    let title: String
    let status: Status

    public init(kind: String, title: String, status: Status) {
        self.kind = kind
        self.title = title
        self.status = status
    }

    public var body: some View {
        HStack(spacing: Tokens.Space.sm) {
            Image(systemName: Self.symbol(kind))
                .frame(width: Tokens.Size.icon)
                .foregroundStyle(.textTertiary)
            Text(title)
                .font(Tokens.Text.codeSmall)
                .foregroundStyle(status == .failed ? .statusFailed : .textSecondary)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
            switch status {
            case .running: ProgressView().controlSize(.mini)
            case .failed: Image(systemName: "xmark").foregroundStyle(.statusFailed)
            case .done: EmptyView()
            }
        }
        .font(Tokens.Text.meta)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Tool kinds are strings from the agent, so unknown ones get a generic symbol.
    static func symbol(_ kind: String) -> String {
        switch kind {
        case "edit": "pencil"
        case "execute": "terminal"
        case "read": "doc.text"
        case "search": "magnifyingglass"
        case "fetch": "globe"
        case "think": "brain"
        case "delete": "trash"
        case "move": "arrow.right.doc.on.clipboard"
        default: "wrench.and.screwdriver"
        }
    }
}

public struct DiffLine: Hashable, Sendable {
    public enum Kind: Sendable { case context, added, removed }

    public let kind: Kind
    public let text: String

    public init(_ kind: Kind, _ text: String) {
        self.kind = kind
        self.text = text
    }
}

public struct DiffHunk: View {
    let header: String
    let lines: [DiffLine]

    public init(header: String, lines: [DiffLine]) {
        self.header = header
        self.lines = lines
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(header)
                .font(Tokens.Text.codeSmall)
                .foregroundStyle(.textTertiary)
                .padding(.horizontal, Tokens.Space.md)
                .padding(.vertical, Tokens.Space.sm)
            ScrollView(.horizontal, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                        row(line)
                    }
                }
            }
        }
        .background(.surfaceRaised, in: .rect(cornerRadius: Tokens.Radius.sm))
        .clipShape(.rect(cornerRadius: Tokens.Radius.sm))
    }

    private func row(_ line: DiffLine) -> some View {
        let (sign, text, background): (String, ThemeColor, ThemeColor?) = switch line.kind {
        case .added: ("+", .diffAddText, .diffAddBg)
        case .removed: ("-", .diffDelText, .diffDelBg)
        case .context: (" ", .syntaxPlain, nil)
        }
        return Text("\(sign) \(line.text)")
            .font(Tokens.Text.codeSmall)
            .foregroundStyle(text)
            .fixedSize()
            .padding(.horizontal, Tokens.Space.md)
            .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
            .background { if let background { Rectangle().fill(background) } }
    }
}

#Preview {
    ScrollView {
        VStack(alignment: .leading, spacing: Tokens.Space.lg) {
            PromptView("Cap the reconnect backoff at 30s and add `jitter`", images: 1)
            ActivityLine(.finished("Worked for 12s, 6 steps", failed: 1), expanded: true)
            ToolCallRow(kind: "read", title: "crates/engine/src/connection.rs", status: .done)
            ToolCallRow(kind: "execute", title: "cargo test -p engine", status: .failed)
            DiffHunk(header: "crates/engine/src/connection.rs", lines: [
                DiffLine(.context, "let delay = base * 2u32.pow(attempt);"),
                DiffLine(.removed, "sleep(delay).await;"),
                DiffLine(.added, "sleep(delay.min(MAX_BACKOFF)).await;"),
            ])
            MessageView([
                .heading("Backoff"),
                .paragraph("The delay now stops at **30s**, and each wait adds up to `250ms` of jitter."),
                .bullets(["Capped at `MAX_BACKOFF`", "Jitter from the thread RNG"]),
                .code(language: "rust", text: "const MAX_BACKOFF: Duration = Duration::from_secs(30);"),
            ])
            ActivityLine(.running("Editing tests/reconnect.rs"))
            ActivityLine(.waiting)
        }
        .padding()
    }
    .background(.surfaceBase)
    .tectrTheme()
}
