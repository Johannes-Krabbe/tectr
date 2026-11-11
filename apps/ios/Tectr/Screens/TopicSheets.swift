import SwiftUI
import TectrDesign

struct MoveTopicSheet: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    let topicID: Topic.ID
    /// A move that would lose work, waiting for a yes.
    @State private var risky: (target: MoveTarget, risk: ArchiveRisk)?

    var body: some View {
        NavigationStack {
            List {
                if let topic = store.topic(topicID) {
                    Section {
                        ForEach(topic.worktrees.filter { !$0.isRepo }) { worktree in
                            row(worktree.branch, detail: worktree.path, current: worktree.id == topic.worktreeID, to: .worktree(worktree.id))
                        }
                        row("New worktree", detail: "A new branch from where it is now", current: false, to: .newWorktree)
                    }
                    Section {
                        row("The repo itself", detail: "On whatever it has checked out", current: topic.worktree?.isRepo == true, to: .root)
                    }
                }
            }
            .navigationTitle("Move to another folder")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(role: .cancel) { dismiss() } }
            }
            .confirmationDialog(
                "Leave work behind?",
                isPresented: Binding { risky != nil } set: { if !$0 { risky = nil } },
                titleVisibility: .visible,
                presenting: risky
            ) { risky in
                Button("Move anyway", role: .destructive) { move(to: risky.target, force: true) }
            } message: { risky in
                Text(risky.risk.losses.joined(separator: "\n"))
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func move(to target: MoveTarget, force: Bool) {
        Task {
            if let risk = await store.move(topicID, to: target, force: force) {
                risky = (target, risk)
            } else {
                dismiss()
            }
        }
    }

    private func row(_ title: String, detail: String, current: Bool, to target: MoveTarget) -> some View {
        Button {
            move(to: target, force: false)
        } label: {
            HStack {
                VStack(alignment: .leading, spacing: Tokens.Space.xxs) {
                    Text(title).foregroundStyle(.primary)
                    Text(detail).font(.subheadline).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
                }
                Spacer()
                if current {
                    Image(systemName: "checkmark").foregroundStyle(.tint)
                }
            }
        }
        .disabled(current)
    }
}

struct ImportConversationSheet: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    let topicID: Topic.ID
    let onImported: (Tab.ID) -> Void
    @State private var query = ""
    @State private var loading = true

    var body: some View {
        let conversations = store.conversations.filter { query.isEmpty || $0.title.localizedStandardContains(query) }
        let device = store.device(of: topicID)
        NavigationStack {
            List {
                Section {
                    ForEach(conversations) { conversation in
                        Button {
                            Task {
                                if let tab = await store.importConversation(conversation, into: topicID) {
                                    onImported(tab)
                                    dismiss()
                                }
                            }
                        } label: {
                            HStack(alignment: .firstTextBaseline) {
                                VStack(alignment: .leading, spacing: Tokens.Space.xxs) {
                                    Text(conversation.title).foregroundStyle(.primary)
                                    Text("\(store.provider(conversation.agent, on: device)?.name ?? conversation.agent), \(conversation.cwd)")
                                        .font(.subheadline)
                                        .foregroundStyle(.secondary)
                                }
                                .lineLimit(1)
                                Spacer()
                                Text(conversation.updated).font(.subheadline).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }
            .overlay {
                if loading {
                    ProgressView()
                } else if conversations.isEmpty {
                    ContentUnavailableView("No conversations", systemImage: "bubble.left.and.bubble.right")
                }
            }
            .task {
                await store.loadConversations(on: device?.id)
                loading = false
            }
            .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always))
            .navigationTitle("Import a conversation")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(role: .cancel) { dismiss() } }
            }
        }
    }
}

/// Which of the device's servers this topic's agents get, and the servers it has to itself.
struct TopicMcpSheet: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    let topicID: Topic.ID
    @State private var editing: McpServer?

    var body: some View {
        let device = store.device(of: topicID)
        let servers = device?.mcpServers ?? []
        NavigationStack {
            List {
                if let topic = store.topic(topicID), let device {
                    Section("This device") {
                        ForEach(servers.filter { $0.topicID == nil }) { server in
                            Toggle(isOn: Binding {
                                topic.mcp[server.id] ?? server.enabled
                            } set: {
                                store.setMcp(server, on: $0, for: topicID)
                            }) {
                                ServerLabel(server: server)
                            }
                        }
                    }
                    Section("This topic") {
                        ForEach(servers.filter { $0.topicID == topicID }) { server in
                            Button { editing = server } label: { ServerLabel(server: server) }
                        }
                        Button("Add a server", systemImage: "plus") {
                            editing = McpServer(id: store.newServerID(), name: "", topicID: topicID,
                                                transport: .stdio(command: "", args: [], env: [:]), enabled: true)
                        }
                    }
                    .sheet(item: $editing) { server in
                        McpServerForm(server: server, deviceID: device.id, isNew: !servers.contains { $0.id == server.id })
                    }
                }
            }
            .navigationTitle("MCP servers")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button(role: .close) { dismiss() } }
            }
        }
    }
}

struct ArchivedTopicsSheet: View {
    @Environment(Store.self) private var store
    @Environment(Toasts.self) private var toasts
    @Environment(\.dismiss) private var dismiss
    @State private var loading = true

    var body: some View {
        NavigationStack {
            List {
                ForEach(store.workspaces) { workspace in
                    let topics = store.archived.filter { $0.workspaceID == workspace.id }
                    if !topics.isEmpty {
                        Section("\(workspace.name) on \(store.device(workspace.deviceID)?.name ?? "")") {
                            ForEach(topics) { topic in
                                HStack {
                                    VStack(alignment: .leading, spacing: Tokens.Space.xxs) {
                                        Text(topic.name)
                                        Text("Archived \(topic.archived)").font(.subheadline).foregroundStyle(.secondary)
                                    }
                                    Spacer()
                                    Button("Restore") { restore(topic) }
                                        .buttonStyle(.bordered)
                                    .buttonBorderShape(.capsule)

                                }
                                .swipeActions {
                                    Button("Restore", systemImage: "arrow.uturn.backward") { restore(topic) }
                                        .tint(ThemeColor.accentSolid)
                                }
                            }
                        }
                    }
                }
            }
            .overlay {
                if loading {
                    ProgressView()
                } else if store.archived.isEmpty {
                    ContentUnavailableView("No archived topics", systemImage: "archivebox")
                }
            }
            .task {
                await store.loadArchived()
                loading = false
            }
            .refreshable { await store.loadArchived() }
            .navigationTitle("Archived topics")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button(role: .close) { dismiss() } }
            }
        }
    }

    private func restore(_ topic: ArchivedTopic) {
        Task {
            if await store.restore(topic) { toasts.show("Restored \(topic.name)") }
        }
    }
}

/// A server's name, and how agents reach it.
struct ServerLabel: View {
    let server: McpServer

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.Space.xxs) {
            HStack(spacing: Tokens.Space.sm) {
                Text(server.name.isEmpty ? "Unnamed" : server.name).foregroundStyle(.primary)
                if server.auth == .needsLogin {
                    Text("Needs login").font(.subheadline).foregroundStyle(.statusWaiting)
                }
            }
            Text(server.transport.summary)
                .font(Tokens.Text.codeSmall)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }
}
