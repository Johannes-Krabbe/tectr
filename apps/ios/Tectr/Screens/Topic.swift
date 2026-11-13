import SwiftUI
import TectrDesign

/// A topic shows one tab at a time. The title menu switches between them, which is how
/// iOS offers what the desktop's tab strip does.
struct TopicView: View {
    @Environment(Store.self) private var store
    let topicID: Topic.ID
    @State private var tabID: Tab.ID?
    @State private var action: TopicAction?
    @State private var newTab: NewTabKind?

    var body: some View {
        if let topic = store.topic(topicID) {
            let tab = topic.tabs.first { $0.id == tabID } ?? topic.tabs.first
            let online = store.isOnline(topic.id)
            content(topic: topic, tab: tab)
                .navigationTitle(topic.name)
                .navigationSubtitle(subtitle(topic: topic, tab: tab))
                .navigationBarTitleDisplayMode(.inline)
                .toolbarTitleMenu {
                    TabMenuItems(topic: topic, selection: tab?.id, online: online, select: { tabID = $0 }, open: open)
                }
                .toolbar {
                    if online {
                        ToolbarItem(placement: .topBarTrailing) {
                            Menu("More", systemImage: "ellipsis") {
                                if let tab { TabActionItems(topic: topic, tab: tab, select: { tabID = $0 }) }
                                TopicMenuItems(topic: topic, action: $action)
                            }
                        }
                    }
                }
                .topicActions($action) { tabID = $0 }
                .sheet(item: $newTab) { kind in
                    Group {
                        switch kind {
                        case .agent:
                            NewAgentSheet(topicID: topic.id, setup: AgentSetup(device: store.device(of: topic.id))) { tabID = $0 }
                        case .importConversation: ImportConversationSheet(topicID: topic.id) { tabID = $0 }
                        }
                    }
                    .toasts()
                }
        }
    }

    @ViewBuilder
    private func content(topic: Topic, tab: Tab?) -> some View {
        switch tab {
        case .agent(let session):
            AgentTabView(session: session, topicID: topic.id) { tabID = $0 }
                // Each tab keeps its own draft and scroll position.
                .id(session.id)
        case .terminal(let terminal):
            TerminalTabView(terminal: terminal, topicID: topic.id)
                .id(terminal.id)
        case nil:
            NewTabView(online: store.isOnline(topic.id), open: open)
        }
    }

    private func subtitle(topic: Topic, tab: Tab?) -> String {
        [tab?.title, topic.worktree?.branch].compactMap(\.self).joined(separator: ", ")
    }

    private func open(_ kind: TabKind) {
        switch kind {
        case .agent: newTab = .agent
        case .terminal:
            Task {
                if let id = await store.openTerminal(in: topicID) { tabID = id }
            }
        case .importConversation: newTab = .importConversation
        case .git, .changes: break
        }
    }

}

enum NewTabKind: Identifiable {
    case agent, importConversation

    var id: Self { self }
}

/// What a new tab can be. Kinds that aren't built yet are shown disabled, as on the desktop.
enum TabKind: CaseIterable, Identifiable {
    case agent, terminal, git, changes, importConversation

    var id: Self { self }

    var label: String {
        switch self {
        case .agent: "Agent"
        case .terminal: "Terminal"
        case .git: "Git"
        case .changes: "Changes"
        case .importConversation: "Import a conversation"
        }
    }

    var symbol: String {
        switch self {
        case .agent: "sparkle"
        case .terminal: "apple.terminal"
        case .git: "arrow.triangle.branch"
        case .changes: "plus.forwardslash.minus"
        case .importConversation: "square.and.arrow.down"
        }
    }

    var isBuilt: Bool { self != .git && self != .changes }
}

private struct TabMenuItems: View {
    let topic: Topic
    let selection: Tab.ID?
    let online: Bool
    let select: (Tab.ID) -> Void
    let open: (TabKind) -> Void

    var body: some View {
        Section {
            ForEach(topic.tabs) { tab in
                Button { select(tab.id) } label: {
                    switch tab {
                    case .agent(let session):
                        Label { Text(tab.title) } icon: { session.glyph.image }
                    case .terminal:
                        Label(tab.title, systemImage: "apple.terminal")
                    }
                }
                .disabled(tab.id == selection)
            }
        }
        if online {
            Menu("New tab", systemImage: "plus") {
                ForEach(TabKind.allCases) { kind in
                    Button(kind.label, systemImage: kind.symbol) { open(kind) }
                        .disabled(!kind.isBuilt)
                }
            }
        }
    }
}

/// What can be done to the tab that's showing.
private struct TabActionItems: View {
    @Environment(Store.self) private var store
    let topic: Topic
    let tab: Tab
    let select: (Tab.ID) -> Void

    var body: some View {
        Section {
            if case .agent(let session) = tab {
                if store.provider(session.agent, on: store.device(of: topic.id))?.canFork == true {
                    Button("Fork", systemImage: "arrow.branch") {
                        Task {
                            if let id = await store.fork(session.id) { select(id) }
                        }
                    }
                }
                if session.status == .done {
                    Button(session.unread ? "Mark read" : "Mark unread", systemImage: "checkmark.circle") {
                        store.setUnread(!session.unread, session: session.id)
                    }
                }
            }
            Button("Close tab", systemImage: "xmark") { store.close(tab.id, in: topic.id) }
                .keyboardShortcut("w", modifiers: .command)
        }
    }
}

/// An empty topic shows what to open, like the desktop's new tab.
struct NewTabView: View {
    let online: Bool
    let open: (TabKind) -> Void

    var body: some View {
        List {
            Section {
                ForEach(TabKind.allCases) { kind in
                    Button { open(kind) } label: {
                        HStack {
                            Label(kind.label, systemImage: kind.symbol)
                            Spacer()
                            if !kind.isBuilt {
                                Text("Not built yet").font(Tokens.Text.meta).foregroundStyle(.textTertiary)
                            }
                        }
                    }
                    .disabled(!online || !kind.isBuilt)
                    .tectrRow()
                }
            } header: {
                Text("New tab")
            } footer: {
                if !online { Text("This device is offline, so nothing can be opened there") }
            }
        }
        .tectrList()
    }
}
