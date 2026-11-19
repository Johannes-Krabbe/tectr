import SwiftUI
import TectrDesign

/// A server's name, how agents reach it, and anything that needs doing before they can.
struct ServerLabel: View {
    let server: McpServer
    var note: String?

    var body: some View {
        VStack(alignment: .leading, spacing: Tokens.Space.xxs) {
            HStack(spacing: Tokens.Space.sm) {
                Text(server.name.isEmpty ? "Unnamed" : server.name).foregroundStyle(.primary)
                if let note {
                    Text(note).font(.subheadline).foregroundStyle(.statusWaiting)
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

extension McpServer {
    /// Why agents can't use it with this profile yet, if they can't.
    func loginNote(_ profile: Profile?) -> String? {
        if let profile { return profile.auth == .needsLogin ? "Needs login" : nil }
        return needsLogin ? "Needs a profile" : nil
    }

    /// For the library, where no profile is chosen: whether any profile works yet.
    var libraryNote: String? {
        needsLogin && !profiles.contains { $0.auth == .loggedIn } ? "Needs login" : nil
    }
}

/// One library server: its profiles, and editing or removing it.
struct McpServerView: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    let serverID: McpServer.ID
    let deviceID: Device.ID
    @State private var editing = false
    @State private var profile: ProfileTarget?

    var body: some View {
        if let device = store.device(deviceID), let server = device.mcpServers.first(where: { $0.id == serverID }) {
            List {
                Section {
                    Button { editing = true } label: { ServerLabel(server: server) }
                }
                Section {
                    ForEach(server.profiles) { item in
                        Button { profile = ProfileTarget(server: server, profile: item) } label: {
                            LabeledContent(item.name, value: Self.status(item, of: server))
                        }
                        .swipeActions {
                            Button("Remove", systemImage: "trash", role: .destructive) {
                                store.remove(profile: item.id, server: server.id, on: deviceID)
                            }
                        }
                    }
                    Button("New profile", systemImage: "plus") { profile = ProfileTarget(server: server, profile: nil) }
                } header: {
                    Text("Profiles")
                } footer: {
                    Text(server.logsIn
                         ? "Each profile is its own login, like a personal and a work account. Workspaces choose one."
                         : "Each profile lays its values over the server's environment. Workspaces choose one.")
                }
                Section {
                    Button("Remove", role: .destructive) {
                        store.remove(server: server.id, on: deviceID)
                        dismiss()
                    }
                }
            }
            .disabled(!device.online)
            .navigationTitle(server.name)
            .navigationBarTitleDisplayMode(.inline)
            .sheet(isPresented: $editing) {
                McpServerForm(server: server, deviceID: deviceID, isNew: false).toasts()
            }
            .sheet(item: $profile) { target in
                McpProfileForm(server: target.server, profile: target.profile, deviceID: deviceID).toasts()
            }
        }
    }

    private static func status(_ profile: McpServer.Profile, of server: McpServer) -> String {
        switch profile.auth {
        case .loggedIn: "Logged in"
        case .needsLogin: "Needs login"
        case .none: server.logsIn ? "" : "\(profile.values.count) values"
        }
    }
}

/// A profile being added or changed, for `.sheet(item:)`.
struct ProfileTarget: Identifiable {
    let server: McpServer
    let profile: McpServer.Profile?
    var id: String { "\(server.id)/\(profile?.id ?? "new")" }
}

/// Adds or changes a profile. A new profile of an HTTP server logs in as soon as it's saved,
/// because a profile is only good for something once it has a login.
struct McpProfileForm: View {
    @Environment(Store.self) private var store
    @Environment(Toasts.self) private var toasts
    @Environment(\.dismiss) private var dismiss
    let server: McpServer
    let deviceID: Device.ID
    /// Hears the ID of a profile once the device has saved it.
    var onSaved: (McpServer.Profile.ID) -> Void = { _ in }
    private let profileID: McpServer.Profile.ID?
    @State private var name: String
    /// `KEY=VALUE`, one to a line.
    @State private var values: String
    @State private var saving = false
    @State private var login: LoginPage?

    init(
        server: McpServer, profile: McpServer.Profile?, deviceID: Device.ID,
        onSaved: @escaping (McpServer.Profile.ID) -> Void = { _ in }
    ) {
        self.server = server
        self.deviceID = deviceID
        self.onSaved = onSaved
        profileID = profile?.id
        _name = State(initialValue: profile?.name ?? "")
        _values = State(initialValue: KeyValueLines.lines(profile?.values ?? [:]))
    }

    private var current: McpServer.Profile? {
        store.device(deviceID)?.mcpServers.first { $0.id == server.id }?.profile(profileID)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name, like personal or work", text: $name)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section {
                    TextField("KEY=VALUE", text: $values, axis: .vertical)
                        .font(Tokens.Text.codeBody)
                        .lineLimit(3...10)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } header: {
                    Text(server.logsIn ? "Headers" : "Environment")
                } footer: {
                    Text("Laid over the server's own. Values can name the daemon's environment as ${NAME}.")
                }
                if server.logsIn, let current {
                    Section("Login") {
                        if current.auth == .loggedIn {
                            LabeledContent("Status", value: "Logged in")
                            Button("Log out", role: .destructive) {
                                store.logout(server: server.id, profile: current.id, on: deviceID)
                            }
                        } else {
                            Button("Log in") { Task { await logIn(current.id) } }
                        }
                    }
                }
                if let profileID {
                    Section {
                        Button("Remove", role: .destructive) {
                            store.remove(profile: profileID, server: server.id, on: deviceID)
                            dismiss()
                        }
                    }
                }
            }
            .navigationTitle(profileID == nil ? "New profile for \(server.name)" : name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(role: .cancel) { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(profileID == nil && server.logsIn ? "Log in" : "Save", systemImage: "checkmark", role: .confirm) {
                        Task { await save() }
                    }
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || saving)
                    .keyboardShortcut(.return, modifiers: .command)
                }
            }
            .sheet(item: $login, onDismiss: { if profileID == nil { dismiss() } }) { page in
                McpLoginView(url: page.url, deviceID: deviceID)
            }
        }
    }

    private func save() async {
        saving = true
        defer { saving = false }
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        guard let saved = await store.saveProfile(
            profileID, named: trimmed, values: KeyValueLines.parse(values), server: server.id, on: deviceID
        ) else { return }
        onSaved(saved)
        if profileID == nil && server.logsIn {
            await logIn(saved)
        } else {
            dismiss()
        }
    }

    private func logIn(_ id: McpServer.Profile.ID) async {
        login = await store.login(server: server.id, profile: id, on: deviceID).map(LoginPage.init)
    }
}

/// `KEY=VALUE` pairs, one to a line, for the boxes that edit an environment or headers.
enum KeyValueLines {
    static func lines(_ pairs: [String: String]) -> String {
        pairs.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" }.joined(separator: "\n")
    }

    static func parse(_ text: String) -> [String: String] {
        var result: [String: String] = [:]
        for line in text.split(separator: "\n") {
            let parts = line.split(separator: "=", maxSplits: 1)
            if parts.count == 2 { result[String(parts[0]).trimmingCharacters(in: .whitespaces)] = String(parts[1]) }
        }
        return result
    }
}

/// The library servers a workspace's agents get, each on or off and with its profile.
struct WorkspaceMcpSheet: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    let workspaceID: Workspace.ID
    /// A server getting a new profile from here, which the workspace then uses.
    @State private var newProfile: McpServer?

    var body: some View {
        NavigationStack {
            Group {
                if let workspace = store.workspace(workspaceID), let device = store.device(workspace.deviceID) {
                    list(workspace, device: device)
                }
            }
            .navigationTitle("MCP servers")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button(role: .close) { dismiss() } }
            }
        }
    }

    private func list(_ workspace: Workspace, device: Device) -> some View {
        let added = device.mcpServers.filter { workspace.mcp[$0.id] != nil }
        let others = device.mcpServers.filter { workspace.mcp[$0.id] == nil }
        let copyable = store.workspaces(on: device).filter { $0.id != workspace.id && !$0.mcp.isEmpty }
        return List {
            Section {
                ForEach(added) { server in
                    row(server, in: workspace)
                        .swipeActions {
                            Button("Remove", systemImage: "minus.circle", role: .destructive) {
                                var servers = workspace.mcp
                                servers[server.id] = nil
                                store.setMcp(servers, for: workspace)
                            }
                        }
                }
                if added.isEmpty {
                    Text("No servers yet").foregroundStyle(.secondary)
                }
            } header: {
                Text(workspace.name)
            }
            Section {
                Menu {
                    ForEach(others) { server in
                        Button(server.name) {
                            var servers = workspace.mcp
                            // One profile is the only choice there is, so it's chosen.
                            let only = server.profiles.count == 1 ? server.profiles.first?.id : nil
                            servers[server.id] = WorkspaceMcp(enabled: true, profileId: only)
                            store.setMcp(servers, for: workspace)
                        }
                    }
                } label: {
                    Label("Add a server", systemImage: "plus")
                }
                .disabled(others.isEmpty)
                if !copyable.isEmpty {
                    Menu {
                        ForEach(copyable) { other in
                            // Adds what this workspace doesn't use yet, like the desktop and CLI.
                            Button(other.name) {
                                store.setMcp(other.mcp.merging(workspace.mcp) { _, mine in mine }, for: workspace)
                            }
                        }
                    } label: {
                        Label("Copy from a workspace", systemImage: "doc.on.doc")
                    }
                }
            } footer: {
                Text("Servers come from \(device.name)'s library, in its settings.")
            }
        }
        .disabled(!device.online)
        .sheet(item: $newProfile) { server in
            McpProfileForm(server: server, profile: nil, deviceID: device.id) { profileID in
                var servers = workspace.mcp
                servers[server.id] = WorkspaceMcp(enabled: servers[server.id]?.enabled ?? true, profileId: profileID)
                store.setMcp(servers, for: workspace)
            }
            .toasts()
        }
    }

    private func row(_ server: McpServer, in workspace: Workspace) -> some View {
        let entry = workspace.mcp[server.id] ?? WorkspaceMcp(enabled: true, profileId: nil)
        let profile = server.profile(entry.profileId)
        return VStack(alignment: .leading, spacing: Tokens.Space.sm) {
            Toggle(isOn: Binding { entry.enabled } set: { on in
                var servers = workspace.mcp
                servers[server.id] = WorkspaceMcp(enabled: on, profileId: entry.profileId)
                store.setMcp(servers, for: workspace)
            }) {
                ServerLabel(server: server, note: server.loginNote(profile))
            }
            if !server.transport.isConnector {
                Menu {
                    Picker("Profile", selection: Binding { entry.profileId } set: { id in
                        var servers = workspace.mcp
                        servers[server.id] = WorkspaceMcp(enabled: entry.enabled, profileId: id)
                        store.setMcp(servers, for: workspace)
                    }) {
                        Text("No profile").tag(McpServer.Profile.ID?.none)
                        ForEach(server.profiles) { item in
                            Text(item.name).tag(Optional(item.id))
                        }
                    }
                    Button("New profile", systemImage: "plus") { newProfile = server }
                } label: {
                    Label(profile?.name ?? "No profile", systemImage: "person.crop.circle")
                        .font(.subheadline)
                }
            }
        }
    }
}
