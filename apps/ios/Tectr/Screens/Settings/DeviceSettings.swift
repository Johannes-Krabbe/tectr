import SwiftUI
import TectrCore
import TectrDesign

/// A device's own settings, kept by its daemon, so they're the same on every client.
struct DeviceView: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    let deviceID: Device.ID
    /// The name being typed, until it's sent.
    @State private var name: String?
    @State private var forgetting = false
    /// The wheel for how long to keep it awake is out.
    @State private var timing = false
    @State private var duration: TimeInterval = 60 * 60

    var body: some View {
        if let device = store.device(deviceID) {
            Form {
                Section {
                    LabeledContent("Name") {
                        TextField("hostname", text: Binding { name ?? device.name } set: { name = $0 })
                            .multilineTextAlignment(.trailing)
                            .onSubmit {
                                if let name { store.rename(device: deviceID, to: name) }
                            }
                    }
                    Toggle("Keep awake", isOn: Binding { device.keepAwake } set: { store.setKeepAwake($0, on: deviceID) })
                } footer: {
                    if !device.online {
                        Text("\(device.name) is offline, so nothing can be changed there. \(device.problem ?? "")")
                    } else if let problem = device.problem {
                        Text(problem)
                    }
                }
                .disabled(!device.online)
                keepAwakeFor(device)
                    .disabled(!device.online)
                Section {
                    NavigationLink { ProvidersView(deviceID: deviceID) } label: {
                        LabeledContent("Providers", value: "\(device.providers.count)")
                    }
                    NavigationLink { McpServersView(deviceID: deviceID) } label: {
                        LabeledContent("MCP servers", value: "\(device.mcpServers.filter { $0.topicID == nil }.count)")
                    }
                    NavigationLink { PairedClientsView(deviceID: deviceID) } label: {
                        LabeledContent("Pairing", value: "\(device.pairedClients.count)")
                    }
                }
                .disabled(!device.online)
                Section {
                    Button("Forget this device", role: .destructive) { forgetting = true }
                }
            }
            .navigationTitle(device.name)
            .animation(.default, value: timing)
            .confirmationDialog("Forget \(device.name)?", isPresented: $forgetting, titleVisibility: .visible) {
                Button("Forget", role: .destructive) {
                    Task {
                        await store.forget(device: deviceID)
                        dismiss()
                    }
                }
            } message: {
                Text("This phone stops reaching it, and comes off its paired list. Pairing again needs a new code from it")
            }
        }
    }

    /// Set like a timer in the Clock app, and it counts down the same way.
    private func keepAwakeFor(_ device: Device) -> some View {
        Section {
            if let until = device.awakeUntil, until > .now {
                LabeledContent("Awake for") {
                    Text(timerInterval: Date.now...until, countsDown: true)
                        .monospacedDigit()
                }
                Button("Stop keeping awake") { store.keepAwake(for: nil, on: deviceID) }
            }
            if timing {
                DurationWheel(seconds: $duration)
                Button("Start") {
                    store.keepAwake(for: duration, on: deviceID)
                    timing = false
                }
                Button("Cancel", role: .cancel) { timing = false }
            } else {
                Button(device.awakeUntil == nil ? "Keep awake for a while" : "Change how long") { timing = true }
            }
        } footer: {
            Text("Closing a laptop's lid still sleeps it")
        }
    }
}

struct ProvidersView: View {
    @Environment(Store.self) private var store
    @Environment(Toasts.self) private var toasts
    let deviceID: Device.ID

    var body: some View {
        if let device = store.device(deviceID) {
            List {
                Section("Added") {
                    ForEach(device.providers) { provider in
                        NavigationLink { ProviderView(deviceID: deviceID, agent: provider.agent) } label: {
                            LabeledContent(provider.name, value: provider.version ?? "")
                        }
                    }
                    if device.providers.isEmpty {
                        Text("No providers yet").foregroundStyle(.secondary)
                    }
                }
                if let candidates = store.candidates[deviceID], !candidates.isEmpty {
                    Section("Available") {
                        ForEach(candidates) { candidate in
                            HStack {
                                VStack(alignment: .leading, spacing: Tokens.Space.xxs) {
                                    Text(candidate.name)
                                    if let missing = candidate.missing {
                                        Text(missing).font(.subheadline).foregroundStyle(.secondary)
                                    }
                                }
                                Spacer()
                                Button("Add") { Task { await store.addProvider(candidate, on: deviceID) } }
                                    .buttonStyle(.bordered)
                                    .buttonBorderShape(.capsule)
                                    .disabled(candidate.missing != nil)
                            }
                        }
                    }
                }
            }
            .navigationTitle("Providers")
            .task { await store.detectProviders(on: deviceID) }
            .refreshable { await store.detectProviders(on: deviceID) }
            .toolbar {
                Button("Detect again", systemImage: "arrow.clockwise") {
                    Task {
                        await store.detectProviders(on: deviceID)
                        let found = store.candidates[deviceID]?.count ?? 0
                        toasts.show(found == 1 ? "1 more agent can be added" : "\(found) more agents can be added")
                    }
                }
            }
        }
    }
}

private struct ProviderView: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    let deviceID: Device.ID
    let agent: String
    @State private var removing = false

    var body: some View {
        if let provider = store.provider(agent, on: store.device(deviceID)) {
            let model = provider.model(provider.defaultModel)
            Form {
                Section {
                    LabeledContent("Version", value: provider.version ?? "Unknown")
                    LabeledContent("Permission modes", value: provider.modes.map(\.label).joined(separator: ", "))
                }
                Section("Defaults for new agents") {
                    Picker("Model", selection: Binding { provider.defaultModel } set: {
                        let efforts = provider.model($0)?.efforts ?? []
                        let effort = efforts.contains { $0.id == provider.defaultEffort } ? provider.defaultEffort : efforts.first?.id
                        store.setDefaults(model: $0, effort: effort, for: agent, on: deviceID)
                    }) {
                        ForEach(provider.models) { Text($0.name).tag(Optional($0.id)) }
                    }
                    if let efforts = model?.efforts, !efforts.isEmpty {
                        Picker("Effort", selection: Binding { provider.defaultEffort } set: {
                            store.setDefaults(model: provider.defaultModel, effort: $0, for: agent, on: deviceID)
                        }) {
                            ForEach(efforts) { Text($0.name).tag(Optional($0.id)) }
                        }
                    }
                }
                Section {
                    Button("Remove", role: .destructive) { removing = true }
                }
            }
            .navigationTitle(provider.name)
            .confirmationDialog("Remove \(provider.name)?", isPresented: $removing, titleVisibility: .visible) {
                Button("Remove", role: .destructive) {
                    store.removeProvider(agent, on: deviceID)
                    dismiss()
                }
            } message: {
                Text("New agents on this device can't use it until it's added again")
            }
        }
    }
}

struct McpServersView: View {
    @Environment(Store.self) private var store
    @Environment(Toasts.self) private var toasts
    let deviceID: Device.ID
    @State private var editing: McpServer?

    var body: some View {
        if let device = store.device(deviceID) {
            let servers = device.mcpServers.filter { $0.topicID == nil }
            List {
                Section("Servers") {
                    ForEach(servers.filter { !$0.transport.isConnector }) { server in
                        Button { editing = server } label: {
                            HStack {
                                ServerLabel(server: server)
                                Spacer()
                                if !server.enabled {
                                    Text("Off").font(.subheadline).foregroundStyle(.secondary)
                                }
                            }
                        }
                        .swipeActions {
                            Button("Remove", systemImage: "trash", role: .destructive) { store.remove(server: server.id, on: deviceID) }
                                .disabled(!device.online)
                        }
                    }
                    if !servers.contains(where: { !$0.transport.isConnector }) {
                        Text("No servers yet").foregroundStyle(.secondary)
                    }
                }
                Section("Connectors") {
                    ForEach(servers.filter(\.transport.isConnector)) { server in
                        Toggle(isOn: Binding { server.enabled } set: { on in
                            var changed = server
                            changed.enabled = on
                            store.save(changed, isNew: false, on: deviceID)
                        }) {
                            VStack(alignment: .leading, spacing: Tokens.Space.xxs) {
                                Text(server.name)
                                Text(server.transport.isClaude ? "Claude only" : "Codex only")
                                    .font(.subheadline)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }
            .navigationTitle("MCP servers")
            .toolbar {
                Menu("Add", systemImage: "plus") {
                    Button("Add a server", systemImage: "plus") {
                        editing = McpServer(id: store.newServerID(), name: "", transport: .stdio(command: "", args: [], env: [:]), enabled: true)
                    }
                    Button("Import from Claude Code and Codex", systemImage: "square.and.arrow.down") {
                        Task {
                            if let imported = await store.importMcpServers(on: deviceID) { toasts.show(imported) }
                        }
                    }
                    Button("Refresh connectors", systemImage: "arrow.clockwise") {
                        Task {
                            if let refreshed = await store.refreshConnectors(on: deviceID) { toasts.show(refreshed) }
                        }
                    }
                }
            }
            .sheet(item: $editing) { server in
                McpServerForm(server: server, deviceID: deviceID, isNew: !device.mcpServers.contains { $0.id == server.id })
                    .toasts()
            }
        }
    }
}

private extension McpServer.Transport {
    var isClaude: Bool {
        if case .claudeConnector = self { true } else { false }
    }
}

/// Adds or edits a server that runs as a command or answers at a URL.
struct McpServerForm: View {
    private enum Kind: Hashable { case command, url }

    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    let deviceID: Device.ID
    let isNew: Bool
    @State private var server: McpServer
    @State private var kind: Kind
    @State private var command = ""
    @State private var args = ""
    @State private var url = ""
    @State private var oauthClientID = ""
    /// `KEY=VALUE`, one to a line: the environment for a command, headers for a URL.
    @State private var pairs = ""
    @State private var login: LoginPage?

    init(server: McpServer, deviceID: Device.ID, isNew: Bool) {
        self.deviceID = deviceID
        self.isNew = isNew
        _server = State(initialValue: server)
        switch server.transport {
        case .stdio(let command, let args, let env):
            _kind = State(initialValue: .command)
            _command = State(initialValue: command)
            _args = State(initialValue: args.joined(separator: " "))
            _pairs = State(initialValue: Self.lines(env))
        case .http(let url, let headers, let oauthClientID):
            _kind = State(initialValue: .url)
            _url = State(initialValue: url)
            _pairs = State(initialValue: Self.lines(headers))
            _oauthClientID = State(initialValue: oauthClientID ?? "")
        case .claudeConnector, .codexApp:
            _kind = State(initialValue: .url)
        }
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name", text: $server.name)

                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    Picker("Type", selection: $kind) {
                        Text("Command").tag(Kind.command)
                        Text("URL").tag(Kind.url)
                    }
                    .pickerStyle(.segmented)
                }
                Section {
                    switch kind {
                    case .command:
                        code("Command", text: $command)
                        code("Arguments", text: $args)
                    case .url:
                        code("URL", text: $url)
                        code("OAuth client ID", text: $oauthClientID)
                    }
                } footer: {
                    Text("Values can name the daemon's environment as ${NAME}, which keeps secrets out of tectr")
                }
                Section(kind == .command ? "Environment" : "Headers") {
                    TextField("KEY=VALUE", text: $pairs, axis: .vertical)
                        .font(Tokens.Text.codeBody)
                        .lineLimit(3...10)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section {
                    Toggle(server.topicID == nil ? "On for every topic" : "On", isOn: $server.enabled)
                }
                if kind == .url && !isNew {
                    Section("Login") {
                        switch server.auth {
                        case .loggedIn:
                            LabeledContent("Status", value: "Logged in")
                            Button("Log out", role: .destructive) { store.logout(server: server.id, on: deviceID) }
                        case .needsLogin, .none:
                            if server.auth == .needsLogin {
                                Text("The server turned the daemon away").foregroundStyle(.statusWaiting)
                            }
                            Button("Log in") {
                                Task { login = await store.login(server: server.id, on: deviceID).map(LoginPage.init) }
                            }
                        }
                    }
                }
                if !isNew {
                    Section {
                        Button("Remove", role: .destructive) {
                            store.remove(server: server.id, on: deviceID)
                            dismiss()
                        }
                    }
                }
            }
            .navigationTitle(isNew ? "New server" : server.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                SheetToolbar(confirm: "Save", enabled: !server.name.isEmpty) {
                    var saved = server
                    saved.transport = transport
                    store.save(saved, isNew: isNew, on: deviceID)
                }
            }
            .sheet(item: $login) { page in
                McpLoginView(url: page.url, deviceID: deviceID)
            }
        }
        // The device says how the login went, so the form follows it.
        .onChange(of: store.device(deviceID)?.mcpServers.first { $0.id == server.id }?.auth) { _, auth in
            if let auth { server.auth = auth }
        }
    }

    private func code(_ label: String, text: Binding<String>) -> some View {
        TextField(label, text: text)
            .font(Tokens.Text.codeBody)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
    }

    private var transport: McpServer.Transport {
        switch kind {
        case .command:
            .stdio(command: command, args: args.split(separator: " ").map(String.init), env: Self.parse(pairs))
        case .url:
            .http(url: url, headers: Self.parse(pairs), oauthClientID: oauthClientID.isEmpty ? nil : oauthClientID)
        }
    }

    private static func lines(_ pairs: [String: String]) -> String {
        pairs.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" }.joined(separator: "\n")
    }

    private static func parse(_ text: String) -> [String: String] {
        var result: [String: String] = [:]
        for line in text.split(separator: "\n") {
            let parts = line.split(separator: "=", maxSplits: 1)
            if parts.count == 2 { result[String(parts[0]).trimmingCharacters(in: .whitespaces)] = String(parts[1]) }
        }
        return result
    }
}

struct PairedClientsView: View {
    @Environment(Store.self) private var store
    let deviceID: Device.ID

    var body: some View {
        if let device = store.device(deviceID) {
            List {
                Section {
                    ForEach(device.pairedClients) { client in
                        LabeledContent(client.name, value: "Paired \(client.paired) ago")
                            .swipeActions {
                                Button("Forget", systemImage: "trash", role: .destructive) { store.forget(client.id, on: deviceID) }
                            }
                            .contextMenu {
                                Button("Forget", systemImage: "trash", role: .destructive) { store.forget(client.id, on: deviceID) }
                            }
                    }
                    if device.pairedClients.isEmpty {
                        Text("No paired clients").foregroundStyle(.secondary)
                    }
                } footer: {
                    Text("Pairing another machine starts on \(device.name) itself")
                }
            }
            .navigationTitle("Pairing")
        }
    }
}

/// Pairs with a device by the code it shows, scanned or pasted, then shows the digits to type
/// there until they are.
struct AddDeviceView: View {
    @Environment(Store.self) private var store
    @Environment(Toasts.self) private var toasts
    @Environment(\.dismiss) private var dismiss
    @State private var code = ""
    @State private var failure: String?
    @State private var pairing = false
    @State private var scanning = false
    @State private var asked: PendingPairing?

    var body: some View {
        Form {
            if let asked {
                Section {
                    Text(Self.spaced(asked.digits))
                        .font(Tokens.Text.title)
                        .monospacedDigit()
                        .frame(maxWidth: .infinity)
                        .textSelection(.enabled)
                } footer: {
                    Text("Type these on \(asked.machineName)")
                }
            } else {
                codeForm
            }
        }
        .navigationTitle("Add a device")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .confirmationAction) {
                if pairing {
                    ProgressView()
                } else {
                    Button("Pair", systemImage: "checkmark", role: .confirm, action: pair)
                        .disabled(code.isEmpty)
                }
            }
        }
        // Leaving stops the pairing, so the device stops waiting on this phone.
        .onDisappear { asked?.stop() }
        .fullScreenCover(isPresented: $scanning) {
            CodeScanner { found in
                code = found
                scanning = false
                pair()
            }
            .ignoresSafeArea()
            .overlay(alignment: .topTrailing) {
                Button("Cancel", systemImage: "xmark") { scanning = false }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.glass)
                    .buttonBorderShape(.circle)
                    .padding()
            }
        }
    }

    @ViewBuilder private var codeForm: some View {
        if CodeScanner.isAvailable {
            Section {
                Button("Scan the code", systemImage: "qrcode.viewfinder") { scanning = true }
            } footer: {
                Text("Point the camera at the code the device shows")
            }
        }
        Section {
            TextField("tectr-...", text: $code)
                .font(Tokens.Text.codeBody)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .onSubmit(pair)
            .disabled(pairing)
            // The system's paste button doesn't ask for clipboard access.
            PasteButton(payloadType: String.self) { strings in
                code = strings.first ?? code
            }
        } header: {
            Text("Code")
        } footer: {
            if let failure {
                Text(failure).foregroundStyle(.statusFailed)
            } else {
                Text("On the device, open Settings, then Pairing")
            }
        }
    }

    private func pair() {
        guard !pairing else { return }
        pairing = true
        failure = nil
        Task {
            do {
                if let pending = try await store.pair(code: code) {
                    asked = pending
                    try await store.finishPairing(pending)
                    toasts.show("Paired with \(pending.machineName)")
                    dismiss()
                }
            } catch {
                failure = error.localizedDescription
            }
            asked = nil
            pairing = false
        }
    }

    /// `482913` as `482 913`, which is easier to read off one screen and type on another.
    private static func spaced(_ digits: String) -> String {
        let middle = digits.index(digits.startIndex, offsetBy: digits.count / 2)
        return "\(digits[..<middle]) \(digits[middle...])"
    }
}

/// The hours and minutes wheel of the Clock app's timer.
struct DurationWheel: UIViewRepresentable {
    @Binding var seconds: TimeInterval

    func makeUIView(context: Context) -> UIDatePicker {
        let picker = UIDatePicker()
        picker.datePickerMode = .countDownTimer
        picker.countDownDuration = seconds
        picker.addTarget(context.coordinator, action: #selector(Coordinator.changed(_:)), for: .valueChanged)
        return picker
    }

    func updateUIView(_ picker: UIDatePicker, context: Context) {
        context.coordinator.parent = self
        if picker.countDownDuration != seconds { picker.countDownDuration = seconds }
    }

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    @MainActor
    final class Coordinator: NSObject {
        var parent: DurationWheel

        init(_ parent: DurationWheel) { self.parent = parent }

        @objc func changed(_ picker: UIDatePicker) { parent.seconds = picker.countDownDuration }
    }
}
