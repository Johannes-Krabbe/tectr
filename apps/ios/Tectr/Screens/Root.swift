import SwiftUI
import TectrDesign

/// Sheets that aren't about one topic.
enum AppSheet: Identifiable, Hashable {
    case addWorkspace(Device.ID?)
    case device(Device.ID)
    case archived
    case settings
    case addDevice

    var id: Self { self }
}

/// What the second column shows.
enum Detail: Hashable {
    case topic(Topic.ID)
    case newTopic(Workspace.ID?)
}

/// The device tree, then a topic, like the desktop's sidebar and pane. On iPhone the two
/// collapse into one stack, and on iPad they sit side by side.
struct RootView: View {
    @Environment(Store.self) private var store
    @State private var detail: Detail?
    /// The last detail shown. On iPhone, going back clears the selection while the screen is
    /// still sliding away, and it would turn into the empty state on its way out.
    @State private var shown: Detail?
    @State private var sheet: AppSheet?

    var body: some View {
        NavigationSplitView {
            HomeView(detail: $detail, sheet: $sheet)
        } detail: {
            switch detail ?? shown {
            case .topic(let topicID) where store.topic(topicID) != nil:
                TopicView(topicID: topicID).id(topicID)
            case .newTopic(let workspaceID):
                NewTopicView(workspaceID: workspaceID) { detail = .topic($0) }
                    .id(workspaceID)
            default:
                ContentUnavailableView("No topic", systemImage: "square.stack")
                    .tectrBackground()
            }
        }
        .onChange(of: detail) { _, detail in
            if let detail { shown = detail }
        }
        .toasts()
        .sheet(item: $sheet) { sheet in
            Group {
                switch sheet {
                case .addWorkspace(let deviceID):
                    AddWorkspaceSheet(deviceID: deviceID ?? store.devices.first(where: \.online)?.id)
                case .device(let deviceID):
                    NavigationStack {
                        DeviceView(deviceID: deviceID).toolbar {
                            ToolbarItem(placement: .confirmationAction) { Button(role: .close) { self.sheet = nil } }
                        }
                    }
                case .archived:
                    ArchivedTopicsSheet()
                case .settings:
                    SettingsSheet()
                case .addDevice:
                    NavigationStack { AddDeviceView() }
                }
            }
            .toasts()
        }
    }
}

/// Devices, their workspaces, and the topics in each, as one tree.
struct HomeView: View {
    @Environment(Store.self) private var store
    @Environment(Toasts.self) private var toasts
    @Binding var detail: Detail?
    @Binding var sheet: AppSheet?
    @State private var query = ""
    @State private var action: TopicAction?

    var body: some View {
        List(selection: $detail) {
            if query.isEmpty {
                tree
            } else {
                SearchResults(topics: foundTopics, actions: foundActions, open: { detail = .topic($0) }, run: run)
            }
        }
        .listStyle(.plain)
        .tectrList()
        // The bar stays, holding the sidebar's foot. Hidden, it made a short pull at the top snap
        // back, and pushing a topic, which has one, shifted the screen by its height.
        .navigationBarTitleDisplayMode(.inline)
        .searchable(text: $query, placement: .toolbar, prompt: "Find a topic or command")
        .refreshable { await reconnect() }
        .overlay {
            // Over the list rather than a row in it, so it's centered in the free space and has
            // no separators.
            if !query.isEmpty && foundTopics.isEmpty && foundActions.isEmpty {
                ContentUnavailableView.search(text: query)
            } else if store.devices.isEmpty {
                ContentUnavailableView {
                    Label("No devices yet", systemImage: "laptopcomputer")
                } actions: {
                    Button("Pair a device") { sheet = .addDevice }
                        .buttonStyle(.glassProminent)
                        .tint(ThemeColor.accentSolid)
                }
            }
        }
        .toolbar {
            // The desktop sidebar's foot: settings, then what isn't about one topic.
            ToolbarItem(placement: .topBarLeading) {
                Button("Settings", systemImage: "gearshape") { sheet = .settings }
                    .keyboardShortcut(",", modifiers: .command)
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu("More actions", systemImage: "ellipsis") {
                    AppMenuItems(run: run)
                }
            }
            DefaultToolbarItem(kind: .search, placement: .bottomBar)
        }
        .topicActions($action)
    }

    private var foundTopics: [Topic] {
        store.workspaces.flatMap(\.topics).filter { $0.name.localizedStandardContains(query) }
    }

    private var foundActions: [AppAction] {
        AppAction.allCases.filter { $0.isAvailable(in: store) && $0.label.localizedStandardContains(query) }
    }

    @ViewBuilder
    private var tree: some View {
        ForEach(store.devices) { device in
            Section {
                ForEach(store.workspaces(on: device)) { workspace in
                    workspaceRow(workspace, on: device)
                    ForEach(workspace.topics) { topic in
                        topicRow(topic, online: device.online)
                    }
                    if device.online && workspace.topics.isEmpty {
                        emptyRow("No topics yet", depth: 1) { detail = .newTopic(workspace.id) }
                    }
                }
                if device.online && store.workspaces(on: device).isEmpty {
                    emptyRow("No workspaces yet", depth: 0) { sheet = .addWorkspace(device.id) }
                }
            } header: {
                DeviceHeader(name: device.name, online: device.online, meta: status(device)) {
                    Button("Settings for \(device.name)", systemImage: "gearshape") { sheet = .device(device.id) }
                        .disabled(!device.online)
                    Button("Add a workspace on \(device.name)", systemImage: "plus") { sheet = .addWorkspace(device.id) }
                        .disabled(!device.online)
                }
            }
        }
    }

    private func workspaceRow(_ workspace: Workspace, on device: Device) -> some View {
        HStack {
            Text(workspace.name)
                .font(Tokens.Text.label)
                .foregroundStyle(device.online ? .textSecondary : .textTertiary)
            Spacer()
            if device.online {
                Button("Create a topic in \(workspace.name)", systemImage: "plus") { detail = .newTopic(workspace.id) }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .foregroundStyle(.textTertiary)
            }
        }
        .lineLimit(1)
        .sidebarRow()
    }

    private func topicRow(_ topic: Topic, online: Bool) -> some View {
        let glyph = store.glyph(topic)
        return NavigationLink(value: Detail.topic(topic.id)) {
            TopicRow(name: topic.name, glyph: glyph, meta: store.meta(topic))
        }
        .sidebarRow(glyph, depth: 1)
        .swipeActions(edge: .leading) {
            if online, topic.sessions.contains(where: { $0.status == .done }) {
                let unread = topic.unreadCount > 0
                Button(unread ? "Mark read" : "Mark unread", systemImage: unread ? "checkmark.circle" : "circle.badge") {
                    store.setUnread(!unread, for: topic.id)
                }
                .tint(ThemeColor.accentSolid)
            }
        }
        .swipeActions(edge: .trailing) {
            if online {
                Button("Archive", systemImage: "archivebox") { action = .archive(topic.id) }
                    .tint(ThemeColor.statusFailed)
            }
        }
        .contextMenu {
            if online { TopicMenuItems(topic: topic, action: $action) }
        }
    }

    private func emptyRow(_ label: String, depth: Int, action: @escaping () -> Void) -> some View {
        Button(label, action: action)
            .font(Tokens.Text.label)
            .foregroundStyle(.textTertiary)
            .sidebarRow(depth: depth)
    }

    /// Nothing while it answers. Otherwise why not, and when it last did.
    private func status(_ device: Device) -> String? {
        if device.online { return device.problem }
        if device.problem == "Connecting" { return "Connecting" }
        let state = device.asleep ? "Asleep" : "Offline"
        return device.lastSeen.map { "\(state), seen \($0)" } ?? state
    }

    private func run(_ action: AppAction) {
        switch action {
        case .newTopic: detail = .newTopic(nil)
        case .addWorkspace: sheet = .addWorkspace(nil)
        case .nextRequest:
            if let request = store.waitingRequests.first, let topicID = store.topicID(of: request.sessionID) { detail = .topic(topicID) }
        case .markAllRead:
            store.markAllRead()
        case .archived: sheet = .archived
        case .reconnect: Task { await reconnect() }
        case .addDevice: sheet = .addDevice
        case .settings: sheet = .settings
        }
    }

    /// Tries every device that's away now, and says which are still away once they've had a
    /// moment to answer.
    private func reconnect() async {
        store.reconnect()
        try? await Task.sleep(for: .seconds(2))
        let away = store.devices.filter { !$0.online }.map(\.name)
        if !away.isEmpty { toasts.show("\(away.joined(separator: ", ")) still offline") }
    }
}

/// What isn't about one topic. Only what would do something right now is there.
enum AppAction: CaseIterable, Identifiable {
    case newTopic, addWorkspace, nextRequest, markAllRead, archived, reconnect, addDevice, settings

    var id: Self { self }

    var label: String {
        switch self {
        case .newTopic: "New topic"
        case .addWorkspace: "Add a workspace"
        case .nextRequest: "Next request"
        case .markAllRead: "Mark every topic read"
        case .archived: "Restore an archived topic"
        case .reconnect: "Reconnect"
        case .addDevice: "Add a device"
        case .settings: "Settings"
        }
    }

    var symbol: String {
        switch self {
        case .newTopic: "plus"
        case .addWorkspace: "folder.badge.plus"
        case .nextRequest: "arrow.turn.down.right"
        case .markAllRead: "checkmark.circle"
        case .archived: "archivebox"
        case .reconnect: "arrow.clockwise"
        case .addDevice: "laptopcomputer.and.iphone"
        case .settings: "gearshape"
        }
    }

    @MainActor func isAvailable(in store: Store) -> Bool {
        switch self {
        case .nextRequest: !store.waitingRequests.isEmpty
        case .markAllRead: store.hasUnread
        case .newTopic: store.devices.contains(where: \.online)
        default: true
        }
    }
}

struct AppMenuItems: View {
    @Environment(Store.self) private var store
    let run: (AppAction) -> Void

    var body: some View {
        Section {
            item(.nextRequest)
            item(.markAllRead)
        }
        Section {
            item(.newTopic)
            item(.addWorkspace)
            item(.archived)
        }
        Section {
            item(.reconnect)
            item(.addDevice)
        }
    }

    @ViewBuilder
    private func item(_ action: AppAction) -> some View {
        if action.isAvailable(in: store) {
            Button(action.label, systemImage: action.symbol) { run(action) }
        }
    }
}

/// Search finds topics and actions, the way the desktop's palette does.
private struct SearchResults: View {
    @Environment(Store.self) private var store
    let topics: [Topic]
    let actions: [AppAction]
    let open: (Topic.ID) -> Void
    let run: (AppAction) -> Void

    var body: some View {
        if !topics.isEmpty {
            Section("Topics") {
                ForEach(topics) { topic in
                    Button { open(topic.id) } label: {
                        TopicRow(name: topic.name, glyph: store.glyph(topic), meta: store.meta(topic))
                    }
                    .foregroundStyle(.textPrimary)
                    .tectrRow()
                }
            }
        }
        if !actions.isEmpty {
            Section("Actions") {
                ForEach(actions) { action in
                    Button(action.label, systemImage: action.symbol) { run(action) }
                        .tectrRow()
                }
            }
        }
    }
}
