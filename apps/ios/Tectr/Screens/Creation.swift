import SwiftUI
import TectrDesign

/// Cancel and confirm, as iOS 26 sheets draw them.
struct SheetToolbar: ToolbarContent {
    let confirm: String
    let enabled: Bool
    let action: () -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some ToolbarContent {
        ToolbarItem(placement: .cancellationAction) {
            Button(role: .cancel) { dismiss() }
        }
        ToolbarItem(placement: .confirmationAction) {
            Button(confirm, systemImage: "checkmark", role: .confirm) {
                action()
                dismiss()
            }
            .disabled(!enabled)
            .keyboardShortcut(.return, modifiers: .command)
        }
    }
}

struct NewAgentSheet: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    let topicID: Topic.ID
    let onStart: (Tab.ID) -> Void
    @State private var setup: AgentSetup?
    @State private var starting = false

    /// `setup` is the device's defaults, there before the sheet shows so its rows don't
    /// appear one frame late.
    init(topicID: Topic.ID, setup: AgentSetup?, onStart: @escaping (Tab.ID) -> Void) {
        self.topicID = topicID
        self.onStart = onStart
        _setup = State(initialValue: setup)
    }

    var body: some View {
        NavigationStack {
            Form {
                if let device = store.device(of: topicID) {
                    Section { AgentFields(setup: $setup, device: device) }
                }
            }
            .navigationTitle("New agent")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(role: .cancel) { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Start", systemImage: "checkmark", role: .confirm) {
                        guard let setup else { return }
                        starting = true
                        Task {
                            let started = await store.startAgent(in: topicID, setup)
                            starting = false
                            if let started {
                                onStart(started)
                                dismiss()
                            }
                        }
                    }
                    .disabled(setup == nil || starting)
                    .keyboardShortcut(.return, modifiers: .command)
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}

/// Provider, model, effort, permission mode and first prompt, from what the device offers.
struct AgentFields: View {
    @Binding var setup: AgentSetup?
    let device: Device

    var body: some View {
        let provider = device.providers.first { $0.agent == setup?.agent }
        let model = provider?.model(setup?.model)
        Group {
            Picker("Provider", selection: field(\.agent, "")) {
                ForEach(device.providers) { Text($0.name).tag($0.agent) }
            }
            if let provider {
                Picker("Model", selection: field(\.model, nil)) {
                    ForEach(provider.models) { Text($0.name).tag(Optional($0.id)) }
                }
                if let efforts = model?.efforts, !efforts.isEmpty {
                    Picker("Effort", selection: field(\.effort, nil)) {
                        ForEach(efforts) { Text($0.name).tag(Optional($0.id)) }
                    }
                }
                Picker("Permission mode", selection: field(\.mode, .auto)) {
                    ForEach(provider.modes) { Text($0.label).tag($0) }
                }
            }
            TextField("What should the agent do?", text: field(\.prompt, ""), axis: .vertical)
                .lineLimit(3...8)
        }
        .onChange(of: setup?.agent) { _, agent in
            // A new provider starts from its own defaults, keeping what was typed.
            guard let agent, let provider = device.providers.first(where: { $0.agent == agent }),
                  setup?.model.flatMap(provider.model) == nil else { return }
            var fresh = AgentSetup(provider: provider)
            fresh.prompt = setup?.prompt ?? ""
            setup = fresh
        }
    }

    private func field<Value>(_ path: WritableKeyPath<AgentSetup, Value>, _ fallback: Value) -> Binding<Value> {
        Binding { setup?[keyPath: path] ?? fallback } set: { setup?[keyPath: path] = $0 }
    }
}

/// Adds a repo on a device by browsing its folders, like Files.
struct AddWorkspaceSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var deviceID: Device.ID?

    init(deviceID: Device.ID?) {
        _deviceID = State(initialValue: deviceID)
    }

    var body: some View {
        NavigationStack {
            FolderList(deviceID: $deviceID, path: "~", isRoot: true) { dismiss() }
                .navigationDestination(for: String.self) { path in
                    FolderList(deviceID: $deviceID, path: path, isRoot: false) { dismiss() }
                }
        }
    }
}

private struct FolderList: View {
    @Environment(Store.self) private var store
    @Environment(Toasts.self) private var toasts
    @Binding var deviceID: Device.ID?
    let path: String
    let isRoot: Bool
    /// Closes the whole sheet, where the environment's dismiss would only go back one folder.
    let close: () -> Void
    @State private var query = ""

    var body: some View {
        let listed = store.folders(at: path, on: deviceID)
        let folders = (listed ?? []).filter { query.isEmpty || $0.name.localizedStandardContains(query) }
        List {
            if isRoot {
                Section {
                    Picker("Device", selection: $deviceID) {
                        ForEach(store.devices.filter(\.online)) { Text($0.name).tag(Optional($0.id)) }
                    }
                }
            }
            Section {
                ForEach(folders) { folder in
                    NavigationLink(value: folder.path) {
                        HStack {
                            Label(folder.name, systemImage: folder.isRepo ? "folder.fill" : "folder")
                            Spacer()
                            if folder.isRepo {
                                Button("Add") { add(folder.path) }
                                    .buttonStyle(.bordered)
                                    .buttonBorderShape(.capsule)

                            }
                        }
                    }
                }
                if listed == nil {
                    ProgressView().frame(maxWidth: .infinity)
                } else if folders.isEmpty {
                    Text("No folders").foregroundStyle(.secondary)
                }
            }
        }
        .task(id: deviceID) { await store.loadFolders(at: path, on: deviceID) }
        .refreshable { await store.loadFolders(at: path, on: deviceID) }
        .searchable(text: $query, prompt: "Filter")
        .navigationTitle(isRoot ? "Add a workspace" : (path as NSString).lastPathComponent)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if isRoot {
                ToolbarItem(placement: .cancellationAction) {
                    Button(role: .cancel) { close() }
                }
            }
            if isRepo {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add", systemImage: "checkmark", role: .confirm) { add(path) }
                }
            }
        }
    }

    /// Whether this folder is a repo, which the listing of the folder above it says.
    private var isRepo: Bool {
        let parent = (path as NSString).deletingLastPathComponent
        return store.folders(at: parent, on: deviceID)?.contains { $0.path == path && $0.isRepo } ?? false
    }

    private func add(_ path: String) {
        guard let deviceID else { return }
        Task {
            guard await store.addWorkspace(on: deviceID, path: path) else { return }
            toasts.show("Added \((path as NSString).lastPathComponent)")
            close()
        }
    }
}
