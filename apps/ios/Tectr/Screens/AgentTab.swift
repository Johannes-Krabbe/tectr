import SwiftUI
import TectrDesign

struct AgentTabView: View {
    @Environment(Store.self) private var store
    @AppStorage(SettingKey.steps) private var steps = StepsDisplay.folded
    @AppStorage(SettingKey.thumbnailsInTranscript) private var thumbnails = true
    let session: Session
    let topicID: Topic.ID
    /// Shows a tab this one was forked into.
    var onFork: (Tab.ID) -> Void = { _ in }
    /// Activities unfolded against the setting, by ID.
    @State private var toggled: Set<Activity.ID> = []
    /// The message box's attach panel and pickers, shown from here rather than the bottom bar.
    @State private var attaching: Attaching?
    @State private var picked: [PreparedImage] = []

    var body: some View {
        let items = store.items(for: session.id)
        Group {
            // The scroll view only comes once the history has, so it's laid out from its end
            // with every row there. Made earlier, it would have to jump to an end it only knows
            // from estimated row heights, and land short or past it.
            if let items {
                transcript(items)
                    .transition(.opacity)
            } else {
                Color.clear
            }
        }
        .animation(.easeOut(duration: Tokens.Motion.fast), value: items == nil)
        .tectrBackground()
        .safeAreaBar(edge: .bottom) {
            if store.isOnline(topicID) {
                // A request takes the message box's place until it's answered, so the
                // transcript stays readable and nothing covers it.
                Group {
                    if let request = store.requests.first(where: { $0.sessionID == session.id }) {
                        RequestBar(request: request)
                    } else {
                        Composer(session: session, topicID: topicID, attaching: $attaching, picked: $picked)
                            .transition(.move(edge: .bottom).combined(with: .opacity))
                    }
                }
                .animation(.smooth(duration: Tokens.Motion.normal), value: pendingRequest)
            }
        }
        .attachPanel($attaching) { picked.append($0) }
        .sensoryFeedback(.success, trigger: pendingRequest) { old, _ in old != nil }
        .onAppear { store.open(session.id) }
        .onDisappear { store.close(session.id) }
        // Looking at a finished turn reads it, here as on the desktop.
        .onChange(of: session.unread, initial: true) { _, unread in
            if unread { store.markRead(session.id) }
        }
    }

    private func transcript(_ items: [TranscriptItem]) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: Tokens.Space.lg) {
                ForEach(items) { item in
                    switch item {
                    case .prompt(_, let text, let images):
                        // Images above the text, as on the desktop.
                        VStack(alignment: .trailing, spacing: Tokens.Space.sm) {
                            if thumbnails && !images.isEmpty {
                                Thumbnails(images: images, topicID: topicID)
                            }
                            PromptView(text)
                        }
                        .frame(maxWidth: .infinity, alignment: .trailing)
                    case .message(_, let blocks, let forkAfter):
                        MessageView(blocks)
                            .contextMenu { messageMenu(blocks, forkAfter: forkAfter) }
                    case .activity(let activity):
                        ActivityView(activity: activity, expanded: isExpanded(activity)) {
                            toggled.formSymmetricDifference([activity.id])
                        }
                    case .note(_, let text, let failed):
                        Text(text)
                            .font(Tokens.Text.meta)
                            .foregroundStyle(failed ? ThemeColor.statusFailed : ThemeColor.textTertiary)
                            .textSelection(.enabled)
                    }
                }
            }
            .padding(Tokens.Space.lg)
            .frame(maxWidth: 760)
            .frame(maxWidth: .infinity)
        }
        // A short transcript sits at the top, so it stays still while a turn streams in under
        // it. A long one opens at its end and keeps following it.
        .defaultScrollAnchor(.top, for: .alignment)
        .defaultScrollAnchor(.bottom, for: .initialOffset)
        .defaultScrollAnchor(.bottom, for: .sizeChanges)
        .scrollDismissesKeyboard(.interactively)
    }

    /// The request being shown. When it changes from one, that one was answered.
    private var pendingRequest: Request.ID? {
        store.requests.first { $0.sessionID == session.id }?.id
    }

    private func isExpanded(_ activity: Activity) -> Bool {
        (steps == .shown) != toggled.contains(activity.id)
    }

    @ViewBuilder
    private func messageMenu(_ blocks: [MessageBlock], forkAfter: UInt64?) -> some View {
        Button("Copy", systemImage: "doc.on.doc") {
            UIPasteboard.general.string = blocks.map(\.plainText).joined(separator: "\n\n")
        }
        if let forkAfter, store.isOnline(topicID),
           store.provider(session.agent, on: store.device(of: topicID))?.canFork == true {
            Button("Fork from here", systemImage: "arrow.branch") {
                Task {
                    if let tab = await store.fork(session.id, after: forkAfter) { onFork(tab) }
                }
            }
        }
    }
}

private extension MessageBlock {
    var plainText: String {
        switch self {
        case .paragraph(let text), .heading(let text): text
        case .bullets(let items): items.map { "- \($0)" }.joined(separator: "\n")
        case .code(_, let text): text
        }
    }
}

/// An activity line, with its steps below it when unfolded.
private struct ActivityView: View {
    let activity: Activity
    let expanded: Bool
    let tap: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.Space.sm) {
            Button(action: tap) {
                ActivityLine(activity.state, expanded: expanded)
            }
            .buttonStyle(.plain)
            if expanded && !activity.steps.isEmpty {
                VStack(alignment: .leading, spacing: Tokens.Space.sm) {
                    ForEach(activity.steps) { step in
                        StepView(step: step)
                    }
                }
                .padding(.leading, Tokens.Space.md)
                .overlay(alignment: .leading) {
                    Rectangle().fill(.lineSubtle).frame(width: 1)
                }
                .transition(.opacity)
            }
        }
        .animation(.smooth(duration: Tokens.Motion.fast), value: expanded)
    }
}

private struct StepView: View {
    let step: Step
    @State private var open = false

    var body: some View {
        let hasDetail = !step.diff.isEmpty || step.output != nil
        VStack(alignment: .leading, spacing: Tokens.Space.sm) {
            Button { open.toggle() } label: {
                ToolCallRow(kind: step.kind, title: step.title, status: step.status)
                    .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .disabled(!hasDetail)
            if open {
                if !step.diff.isEmpty {
                    DiffHunk(header: step.title, lines: step.diff)
                }
                if let output = step.output {
                    CodeBlock(output)
                }
            }
        }
    }
}

/// A prompt's images, small, fetched from its device when they weren't picked here.
private struct Thumbnails: View {
    let images: [PromptImage]
    let topicID: Topic.ID
    @ScaledMetric(relativeTo: .callout) private var size: CGFloat = 64

    var body: some View {
        HStack(spacing: Tokens.Space.sm) {
            ForEach(images, id: \.id) { image in
                RemoteImage(image: image, topicID: topicID, thumbnail: true)
                    .frame(width: size, height: size)
                    .clipShape(.rect(cornerRadius: Tokens.Radius.md))
            }
        }
    }
}

/// An image whose bytes are with a device, blank until they arrive.
struct RemoteImage: View {
    @Environment(Store.self) private var store
    let image: PromptImage
    let topicID: Topic.ID?
    let thumbnail: Bool
    @State private var loaded: UIImage?

    var body: some View {
        Group {
            if let loaded {
                Image(uiImage: loaded).resizable().scaledToFill()
            } else {
                ImagePlaceholder()
            }
        }
        .task(id: image.id) {
            loaded = await store.image(image, in: topicID, thumbnail: thumbnail)
        }
    }
}
