import SwiftUI
import TectrCore
import TectrDesign

/// Starts a topic from its first prompt, as on the desktop: the prompt is the screen, with
/// where it goes above it and the agent below. The topic takes its first tab's title.
struct NewTopicView: View {
    @Environment(Store.self) private var store
    @AppStorage(SettingKey.thumbnailsInBox) private var thumbnails = true
    /// The workspace asked for or picked, which `workspaceID` falls back from.
    @State private var chosen: Workspace.ID?
    /// The repo's own folder, the desktop's default too. A worktree is one tap away.
    @State private var inRepo = true
    /// The agent as picked, which `setup` falls back from on a device without its provider.
    @State private var picked: AgentSetup?
    @State private var prompt = ""
    @State private var attachments = Attachments()
    @State private var starting = false
    @State private var attaching: Attaching?
    @FocusState private var focused: Bool
    let onStart: (Topic.ID) -> Void

    init(workspaceID: Workspace.ID?, onStart: @escaping (Topic.ID) -> Void) {
        _chosen = State(initialValue: workspaceID)
        self.onStart = onStart
    }

    // The workspace and agent are worked out while drawing rather than set on appear, so the
    // screen is complete in its first frame and nothing rearranges during the push.

    /// The chosen workspace, or the first one on a device that's online.
    private var workspaceID: Workspace.ID? {
        if let workspace = store.workspace(chosen), store.device(workspace.deviceID)?.online == true {
            return workspace.id
        }
        return store.devices.filter(\.online).lazy.flatMap { store.workspaces(on: $0) }.first?.id
    }

    /// What was picked, or the defaults of the workspace's device. Another device has its own
    /// providers, so a pick it can't run gives way to its defaults.
    private var setup: AgentSetup? {
        let device = store.device(store.workspace(workspaceID)?.deviceID)
        if let picked, store.provider(picked.agent, on: device) != nil { return picked }
        return AgentSetup(device: device)
    }

    var body: some View {
        let device = store.device(store.workspace(workspaceID)?.deviceID)
        ScrollView {
            VStack(alignment: .leading, spacing: Tokens.Space.lg) {
                HStack(spacing: Tokens.Space.sm) {
                    workspaceMenu
                    placeMenu
                }
                TextField("What should the agent do?", text: $prompt, axis: .vertical)
                    .font(Tokens.Text.reading)
                    .lineLimit(6...)
                    .focused($focused)
                if thumbnails, !images.isEmpty {
                    AttachmentStrip(images: images, topicID: nil) { attachments.remove($0, from: &prompt) }
                }
            }
            .padding(Tokens.Space.lg)
            .frame(maxWidth: 760, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .scrollDismissesKeyboard(.interactively)
        .tectrBackground()
        .foregroundStyle(.textPrimary)
        .contentShape(.rect)
        .onTapGesture { focused = true }
        .navigationTitle("New topic")
        .navigationBarTitleDisplayMode(.inline)
        .safeAreaBar(edge: .bottom) { bar(device: device) }
        .attachPanel($attaching, add: add)
        .onAppear { focused = true }
        .onChange(of: focused) {
            if focused && attaching == .panel { attaching = nil }
        }
    }

    private var images: [PromptImage] { attachments.named(in: prompt) }

    private var canStart: Bool {
        workspaceID != nil && setup != nil && !starting
            && (!prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !images.isEmpty)
    }

    private func start() {
        guard canStart, let workspaceID, var setup else { return }
        if let refused = Engine.refusal(of: prompt) {
            store.report(refused)
            return
        }
        setup.prompt = prompt
        setup.images = images
        starting = true
        Task {
            let topic = await store.createTopic(in: workspaceID, name: "", inRepo: inRepo, agent: setup)
            starting = false
            if let topic { onStart(topic) }
        }
    }

    private func add(_ image: PreparedImage) {
        // A new session's numbers start at 1.
        let picked = store.pick(image.data, mime: image.mime, width: image.width, height: image.height,
                                number: attachments.nextNumber(sent: 0))
        attachments.add(picked, to: &prompt)
    }

    private var workspaceMenu: some View {
        let workspace = store.workspace(workspaceID)
        return Menu {
            Picker("Workspace", selection: Binding { workspaceID } set: { chosen = $0 }) {
                ForEach(store.devices.filter(\.online)) { device in
                    Section(device.name) {
                        ForEach(store.workspaces(on: device)) { Text($0.name).tag(Optional($0.id)) }
                    }
                }
            }
            .pickerStyle(.inline)
        } label: {
            HStack(spacing: Tokens.Space.xs) {
                Text(workspace?.name ?? "Workspace")
                if let device = store.device(workspace?.deviceID) {
                    Text(device.name).foregroundStyle(.textTertiary)
                }
            }
        }
        .chip()
    }

    private var placeMenu: some View {
        Menu {
            Picker("Where to work", selection: $inRepo) {
                Text("Repo root").tag(true)
                Text("New worktree").tag(false)
            }
            .pickerStyle(.inline)
        } label: {
            Label(inRepo ? "Repo root" : "New worktree", systemImage: inRepo ? "folder" : "arrow.triangle.branch")
        }
        .chip()
    }

    private func bar(device: Device?) -> some View {
        HStack(spacing: Tokens.Space.sm) {
            AttachButton(attaching: $attaching) { focused = false }
            if let device {
                AgentSetupMenu(setup: Binding { setup } set: { picked = $0 }, device: device)
            }
            Spacer()
            if starting { ProgressView() }
            Button("Start", action: start)
                .font(Tokens.Text.label)
                .buttonStyle(.glassProminent)
                .tint(ThemeColor.accentSolid)
                .disabled(!canStart)
                .keyboardShortcut(.return, modifiers: .command)
        }
        .padding(.horizontal, Tokens.Space.lg)
        .padding(.bottom, Tokens.Space.sm)
        .frame(maxWidth: 760)
    }
}

/// Provider, model, effort and permission mode for an agent that hasn't started, in one menu.
struct AgentSetupMenu: View {
    @Binding var setup: AgentSetup?
    let device: Device

    var body: some View {
        let provider = device.providers.first { $0.agent == setup?.agent }
        let model = provider?.model(setup?.model)
        Menu {
            Picker("Provider", selection: field(\.agent, "")) {
                ForEach(device.providers) { Text($0.name).tag($0.agent) }
            }
            .pickerStyle(.menu)
            if let provider {
                Picker("Model", selection: field(\.model, nil)) {
                    ForEach(provider.models) { Text($0.name).tag(Optional($0.id)) }
                }
                .pickerStyle(.menu)
                if let efforts = model?.efforts, !efforts.isEmpty {
                    Picker("Effort", selection: field(\.effort, nil)) {
                        ForEach(efforts) { Text($0.name).tag(Optional($0.id)) }
                    }
                    .pickerStyle(.menu)
                }
                Picker("Permission mode", selection: field(\.mode, .auto)) {
                    ForEach(provider.modes) { Text($0.label).tag($0) }
                }
                .pickerStyle(.menu)
            }
        } label: {
            HStack(spacing: Tokens.Space.xs) {
                Text(provider?.name ?? "No providers")
                if let model { Text(model.name).foregroundStyle(.textTertiary) }
            }
            .font(Tokens.Text.meta)
            .padding(.horizontal, Tokens.Space.xs)
        }
        .buttonStyle(.glass)
        .disabled(device.providers.isEmpty)
        .onChange(of: setup?.agent) { _, agent in
            // A new provider starts from its own defaults.
            guard let agent, let provider = device.providers.first(where: { $0.agent == agent }),
                  setup?.model.flatMap(provider.model) == nil else { return }
            setup = AgentSetup(provider: provider)
        }
    }

    private func field<Value>(_ path: WritableKeyPath<AgentSetup, Value>, _ fallback: Value) -> Binding<Value> {
        Binding { setup?[keyPath: path] ?? fallback } set: { setup?[keyPath: path] = $0 }
    }
}

private extension View {
    /// A small control on the content surface, for choices that sit above the prompt.
    func chip() -> some View {
        font(Tokens.Text.meta)
            .buttonStyle(.bordered)
            .buttonBorderShape(.capsule)
            .tint(ThemeColor.textSecondary)
    }
}
