import SwiftUI
import TectrDesign

/// What can be done to a topic. The context menu, the topic's toolbar menu and search all
/// offer the same, as on the desktop.
enum TopicAction: Identifiable, Hashable {
    case rename(Topic.ID)
    case move(Topic.ID)
    case mcp(Topic.ID)
    case importConversation(Topic.ID)
    case archive(Topic.ID)

    var id: Self { self }

    var isSheet: Bool {
        switch self {
        case .move, .mcp, .importConversation: true
        case .rename, .archive: false
        }
    }
}

struct TopicMenuItems: View {
    @Environment(Store.self) private var store
    let topic: Topic
    @Binding var action: TopicAction?

    var body: some View {
        Section {
            Button("Rename", systemImage: "pencil") { action = .rename(topic.id) }
            if topic.sessions.contains(where: { $0.status == .done }) {
                let unread = topic.unreadCount > 0
                Button(unread ? "Mark read" : "Mark unread", systemImage: unread ? "checkmark.circle" : "circle.badge") {
                    store.setUnread(!unread, for: topic.id)
                }
            }
            Button("Move to another folder", systemImage: "arrow.triangle.branch") { action = .move(topic.id) }
        }
        Section {
            Button("MCP servers", systemImage: "puzzlepiece.extension") { action = .mcp(topic.id) }
            Button("Import a conversation", systemImage: "square.and.arrow.down") { action = .importConversation(topic.id) }
        }
        Section {
            Button("Archive", systemImage: "archivebox", role: .destructive) { action = .archive(topic.id) }
        }
    }
}

extension View {
    func topicActions(_ action: Binding<TopicAction?>, onImported: @escaping (Tab.ID) -> Void = { _ in }) -> some View {
        modifier(TopicActionsModifier(action: action, onImported: onImported))
    }
}

private struct TopicActionsModifier: ViewModifier {
    @Environment(Store.self) private var store
    @Binding var action: TopicAction?
    let onImported: (Tab.ID) -> Void
    @State private var name = ""
    /// What archiving would lose, which the device sends back instead of archiving.
    @State private var risk: (topicID: Topic.ID, risk: ArchiveRisk)?

    func body(content: Content) -> some View {
        content
            .sheet(item: sheet) { action in
                Group {
                    switch action {
                    case .move(let id): MoveTopicSheet(topicID: id)
                    case .mcp(let id): TopicMcpSheet(topicID: id)
                    case .importConversation(let id): ImportConversationSheet(topicID: id, onImported: onImported)
                    case .rename, .archive: EmptyView()
                    }
                }
                .toasts()
            }
            .alert("Rename", isPresented: flag { if case .rename = $0 { true } else { false } }) {
                TextField("Name", text: $name)
                Button("Rename") {
                    if case .rename(let id) = action { store.rename(id, to: name) }
                }
                Button("Cancel", role: .cancel) {}
            }
            .confirmationDialog(
                "Archive \(risk.flatMap { store.topic($0.topicID)?.name } ?? "topic")?",
                isPresented: Binding { risk != nil } set: { if !$0 { risk = nil } },
                titleVisibility: .visible,
                presenting: risk
            ) { risk in
                Button("Archive anyway", role: .destructive) {
                    Task { _ = await store.archive(risk.topicID, force: true) }
                }
            } message: { risk in
                Text(([risk.risk.summary] + risk.risk.losses).joined(separator: "\n"))
            }
            .onChange(of: action) { _, action in
                switch action {
                case .rename(let id):
                    name = store.topic(id)?.name ?? ""
                case .archive(let id):
                    // The device archives straight away unless that would lose something,
                    // and then says what, as on the desktop.
                    self.action = nil
                    Task {
                        if let found = await store.archive(id, force: false) { risk = (id, found) }
                    }
                default:
                    break
                }
            }
    }

    private var sheet: Binding<TopicAction?> {
        Binding { action?.isSheet == true ? action : nil } set: { action = $0 }
    }

    private func flag(_ matches: @escaping (TopicAction) -> Bool) -> Binding<Bool> {
        Binding { action.map(matches) ?? false } set: { if !$0 { action = nil } }
    }
}
