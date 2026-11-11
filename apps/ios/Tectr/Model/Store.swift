import CryptoKit
import Foundation
import Observation
import SwiftUI
import TectrCore
import TectrDesign

/// The merged state of every daemon, as the screens show it, and the actions sent to them. It
/// rereads the engine whenever the engine says something changed, as the desktop's bridge does.
/// Previews run it without an engine, on fixed state.
@MainActor @Observable
final class Store {
    private(set) var devices: [Device] = []
    private(set) var workspaces: [Workspace] = []
    private(set) var requests: [Request] = []
    private(set) var archived: [ArchivedTopic] = []
    private(set) var conversations: [Conversation] = []
    /// What each device offers to add, once asked. By device.
    private(set) var candidates: [Device.ID: [ProviderCandidate]] = [:]
    /// What's in folders that have been listed, by device and path.
    private(set) var folders: [String: [Folder]] = [:]
    /// Each open session's transcript, folded from its events.
    private(set) var transcripts: [Session.ID: [TranscriptItem]] = [:]
    /// Every theme there is, from the settings crate's built-in list.
    let themes: [Theme]
    /// Shows what went wrong. The app points this at its toasts.
    @ObservationIgnored var report: (String) -> Void = { _ in }

    @ObservationIgnored private let engine: Engine?
    @ObservationIgnored private var raw: [Wire.Device] = []
    /// The sessions whose events come in, open in a view or kept from one.
    @ObservationIgnored private var following: [Session.ID: Following] = [:]
    /// How many views show each session.
    @ObservationIgnored private var watchers: [Session.ID: Int] = [:]
    /// Sessions no view shows any more, oldest first. They stay open for a while, so going back
    /// to one shows it straight away instead of an empty screen that fills in after the push.
    @ObservationIgnored private var recent: [Session.ID] = []
    private static let keptOpen = 8
    @ObservationIgnored private var draftSaves: [Session.ID: Task<Void, Never>] = [:]
    /// Images picked here, with their bytes, until each device that needs one has it.
    @ObservationIgnored private var picked: [String: (mime: String, data: Data)] = [:]
    @ObservationIgnored private var sentImages: Set<String> = []
    @ObservationIgnored private let imageCache = NSCache<NSString, UIImage>()

    init(engine: Engine) {
        self.engine = engine
        themes = Self.loadThemes()
        engine.listen { [weak self] devices in
            Task { @MainActor in self?.changed(devices: devices) }
        }
        changed(devices: true)
    }

    /// Fixed state, for previews.
    init(
        devices: [Device],
        workspaces: [Workspace],
        requests: [Request],
        archived: [ArchivedTopic],
        conversations: [Conversation],
        candidates: [Device.ID: [ProviderCandidate]],
        folders: [String: [Folder]],
        transcripts: [Session.ID: [TranscriptItem]]
    ) {
        engine = nil
        themes = Self.loadThemes()
        self.devices = devices
        self.workspaces = workspaces
        self.requests = requests
        self.archived = archived
        self.conversations = conversations
        self.candidates = candidates
        self.folders = folders
        self.transcripts = transcripts
    }

    // MARK: Reading the engine

    private func changed(devices changed: Bool) {
        guard let engine else { return }
        if changed {
            raw = engine.devices()
            rebuild()
        }
        for (sessionID, open) in following {
            guard session(sessionID) != nil else {
                // A closed tab's session, which nothing can show again.
                if watchers[sessionID] == nil { forget(sessionID) }
                continue
            }
            let fresh = engine.events(open.machine, sessionID, after: open.fold.lastSeq)
            // Without new events only a status change, which comes with the devices, changes it.
            guard !fresh.isEmpty || changed else { continue }
            following[sessionID]?.fold.append(fresh)
            if open.shown {
                publish(sessionID)
            } else if !fresh.isEmpty {
                settle(sessionID)
            }
        }
    }

    private func publish(_ sessionID: Session.ID) {
        guard let open = following[sessionID], let session = session(sessionID) else { return }
        let items = open.fold.items(status: session.status)
        if transcripts[sessionID] != items { transcripts[sessionID] = items }
    }

    /// The daemon replays a session's history an event at a time, over several updates. It's
    /// shown once it stops arriving, so it lands in one piece instead of scrolling past as it
    /// comes in, and a running session doesn't show its Working line alone first. A busy
    /// session might never stop, so this waits half a second at most.
    private func settle(_ sessionID: Session.ID) {
        guard let open = following[sessionID] else { return }
        open.settling?.cancel()
        let left = open.opened + .milliseconds(500) - ContinuousClock.now
        let wait = min(.milliseconds(80), max(.zero, left))
        following[sessionID]?.settling = Task { [weak self] in
            try? await Task.sleep(for: wait)
            guard !Task.isCancelled, let self else { return }
            following[sessionID]?.shown = true
            publish(sessionID)
        }
    }

    private func rebuild() {
        var devices: [Device] = []
        var workspaces: [Workspace] = []
        var requests: [Request] = []
        for wire in raw {
            let state = wire.state
            devices.append(Device(
                id: wire.url,
                machineID: state?.machine.id,
                name: state?.machine.name ?? Self.placeholderName(wire.url),
                online: wire.connection.isOnline,
                problem: Self.problem(wire.connection),
                lastSeen: wire.lastSeenMs.map(ago),
                keepAwake: state?.settings.keepAwake ?? true,
                providers: state?.providers.map(Provider.init) ?? [],
                mcpServers: state?.mcpServers.map(McpServer.init) ?? [],
                pairedClients: state?.pairedClients.map {
                    PairedClient(id: $0.key, name: $0.name, paired: ago($0.pairedAtMs))
                } ?? []
            ))
            guard let state else { continue }
            for workspace in state.workspaces {
                workspaces.append(Workspace(
                    id: workspace.id, deviceID: wire.url, name: workspace.name, path: workspace.path,
                    topics: workspace.topics.map { Self.topic($0, in: workspace, state: state) }
                ))
            }
            requests += state.approvals.map(Request.init)
        }
        if self.devices != devices { self.devices = devices }
        if self.workspaces != workspaces { self.workspaces = workspaces }
        if self.requests != requests { self.requests = requests }
    }

    private static func topic(_ topic: Wire.Topic, in workspace: Wire.Workspace, state: Wire.DaemonState) -> Topic {
        let providers = Dictionary(state.providers.map { ($0.agent, $0.name) }, uniquingKeysWith: { first, _ in first })
        let sessions = topic.sessions.map { session in
            (at: session.openedAtMs, tab: Tab.agent(Session(
                id: session.id,
                title: session.title ?? providers[session.agent] ?? session.agent,
                agent: session.agent,
                model: session.model,
                effort: session.effort,
                mode: PermissionMode(rawValue: session.mode.rawValue) ?? .auto,
                status: SessionStatus(session.status),
                unread: session.unread,
                draft: state.drafts[session.id] ?? "",
                draftImages: state.draftImages[session.id] ?? [],
                commands: (state.commands[session.id] ?? []).map { SlashCommand(name: $0.name, detail: $0.description) }
            )))
        }
        let terminals = topic.terminals.map { terminal in
            (at: terminal.openedAtMs, tab: Tab.terminal(Terminal(
                id: terminal.id,
                title: terminal.title.flatMap { $0.isEmpty ? nil : $0 } ?? "Terminal",
                status: terminal.status == .running ? .running : terminal.status == .exited ? .exited : .stopped
            )))
        }
        // Tabs opened at the same moment keep the order the daemon lists them in.
        let tabs = (sessions + terminals).enumerated()
            .sorted { ($0.element.at, $0.offset) < ($1.element.at, $1.offset) }
            .map(\.element.tab)
        return Topic(
            id: topic.id, name: topic.name, worktreeID: topic.worktreeId,
            worktrees: topic.worktrees.map {
                Worktree(id: $0.id, path: $0.path, branch: $0.branch, isRepo: $0.path == workspace.path)
            },
            tabs: tabs, mcp: topic.mcp
        )
    }

    private static func placeholderName(_ url: String) -> String {
        guard url.hasPrefix("iroh://") else { return url }
        return "Device \(url.dropFirst("iroh://".count).prefix(8))"
    }

    private static func problem(_ connection: Wire.Connection) -> String? {
        switch connection {
        case .connecting: "Connecting"
        case .online(let stalled): stalled ? "Not responding" : nil
        case .offline(let reason): reason
        case .incompatible(let min, let max): "It speaks protocol \(min) to \(max), which this app doesn't"
        case .unknown: nil
        }
    }

    // MARK: Lookups

    func device(_ id: Device.ID?) -> Device? { devices.first { $0.id == id } }
    func workspace(_ id: Workspace.ID?) -> Workspace? { workspaces.first { $0.id == id } }

    func workspaces(on device: Device) -> [Workspace] {
        workspaces.filter { $0.deviceID == device.id }
    }

    func workspace(containing topicID: Topic.ID?) -> Workspace? {
        workspaces.first { $0.topics.contains { $0.id == topicID } }
    }

    func topic(_ id: Topic.ID?) -> Topic? {
        for workspace in workspaces {
            if let topic = workspace.topics.first(where: { $0.id == id }) { return topic }
        }
        return nil
    }

    func session(_ id: Session.ID) -> Session? {
        topic(topicID(of: id))?.sessions.first { $0.id == id }
    }

    func device(of topicID: Topic.ID?) -> Device? {
        device(workspace(containing: topicID)?.deviceID)
    }

    func isOnline(_ topicID: Topic.ID?) -> Bool { device(of: topicID)?.online ?? false }

    /// What a row shows: offline devices only have their last known state.
    func glyph(_ topic: Topic) -> StatusGlyph {
        isOnline(topic.id) ? topic.glyph : .stale
    }

    /// Only what needs you is worded, as on the desktop.
    func meta(_ topic: Topic) -> String? {
        let glyph = glyph(topic)
        return glyph == .waiting || glyph == .failed ? glyph.label : nil
    }

    var waitingRequests: [Request] {
        requests.filter { isOnline(topicID(of: $0.sessionID)) }
    }

    var hasUnread: Bool { workspaces.flatMap(\.topics).contains { $0.unreadCount > 0 } }

    func topicID(of sessionID: Session.ID) -> Topic.ID? {
        workspaces.flatMap(\.topics).first { $0.sessions.contains { $0.id == sessionID } }?.id
    }

    func provider(_ agent: String, on device: Device?) -> Provider? {
        device?.providers.first { $0.agent == agent }
    }

    /// "1 of 2 waiting", across every reachable device.
    func position(of request: Request) -> String? {
        let all = waitingRequests
        guard all.count > 1, let index = all.firstIndex(of: request) else { return nil }
        return "\(index + 1) of \(all.count) waiting"
    }

    func folders(at path: String, on deviceID: Device.ID?) -> [Folder]? {
        folders[Self.folderKey(path, deviceID)]
    }

    /// Nil until its history has started to arrive.
    func items(for sessionID: Session.ID) -> [TranscriptItem]? {
        transcripts[sessionID]
    }

    private func machineID(of topicID: Topic.ID?) -> String? { device(of: topicID)?.machineID }
    private func machineID(ofSession sessionID: Session.ID) -> String? { machineID(of: topicID(of: sessionID)) }

    private static func folderKey(_ path: String, _ deviceID: Device.ID?) -> String {
        "\(deviceID ?? "")\n\(path)"
    }

    // MARK: Sending

    /// Runs a command, and shows what went wrong if it fails. Nil when it failed.
    @discardableResult
    private func send(_ machineID: String?, _ command: Command) async -> Wire.CommandOutput? {
        guard let engine else { return nil }
        guard let machineID else {
            report("That device is offline")
            return nil
        }
        do {
            return try await engine.request(machineID, command)
        } catch {
            report(error.localizedDescription)
            return nil
        }
    }

    private func fire(_ machineID: String?, _ command: Command) {
        Task { await send(machineID, command) }
    }

    func reconnect() {
        engine?.reconnect()
    }

    // MARK: Sessions

    /// Starts following a session's events, for as long as a view shows it.
    func open(_ sessionID: Session.ID) {
        watchers[sessionID, default: 0] += 1
        recent.removeAll { $0 == sessionID }
        guard following[sessionID] == nil, let engine, let machine = machineID(ofSession: sessionID) else { return }
        engine.openSession(machine, sessionID)
        following[sessionID] = Following(machine: machine)
        changed(devices: false)
    }

    func close(_ sessionID: Session.ID) {
        watchers[sessionID, default: 1] -= 1
        guard watchers[sessionID] == 0 else { return }
        watchers[sessionID] = nil
        recent.append(sessionID)
        while recent.count > Self.keptOpen { forget(recent.removeFirst()) }
    }

    /// Stops following a session no view shows.
    private func forget(_ sessionID: Session.ID) {
        recent.removeAll { $0 == sessionID }
        guard let engine, let open = following.removeValue(forKey: sessionID) else { return }
        open.settling?.cancel()
        transcripts[sessionID] = nil
        engine.closeSession(open.machine, sessionID)
    }

    func send(_ text: String, images: [PromptImage], to sessionID: Session.ID) {
        let machine = machineID(ofSession: sessionID)
        draftSaves[sessionID]?.cancel()
        Task {
            guard await give(images, to: machine) else { return }
            await send(machine, .sendPrompt(sessionID, text: text, images: images))
        }
    }

    func cancel(_ sessionID: Session.ID) {
        fire(machineID(ofSession: sessionID), .cancelTurn(sessionID))
    }

    func answer(_ request: Request, with option: RequestCard.Option) {
        fire(machineID(ofSession: request.sessionID), .answerApproval(request.id, option: option.id))
    }

    /// Saved a moment after typing stops, so every client sees the draft without a command
    /// for every key.
    func setDraft(_ text: String, images: [PromptImage], for sessionID: Session.ID) {
        let machine = machineID(ofSession: sessionID)
        draftSaves[sessionID]?.cancel()
        draftSaves[sessionID] = Task {
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled, await give(images, to: machine) else { return }
            await send(machine, .setDraft(sessionID, text: text, images: images))
        }
    }

    func setSettings(model: String?, effort: String?, mode: PermissionMode, for sessionID: Session.ID) {
        fire(machineID(ofSession: sessionID), .setSessionSettings(sessionID, mode: mode.rawValue, model: model, effort: effort))
    }

    /// Starts an agent, and sends it the setup's prompt if there is one. Nil when it failed.
    func startAgent(in topicID: Topic.ID, _ setup: AgentSetup) async -> Tab.ID? {
        let machine = machineID(of: topicID)
        let started = await send(machine, .startSession(
            topicId: topicID, agent: setup.agent, mode: setup.mode.rawValue, model: setup.model, effort: setup.effort
        ))
        guard case .session(_, let sessionID) = started else { return nil }
        let prompt = setup.prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        if !prompt.isEmpty || !setup.images.isEmpty, await give(setup.images, to: machine) {
            await send(machine, .sendPrompt(sessionID, text: setup.prompt, images: setup.images))
        }
        return sessionID
    }

    func openTerminal(in topicID: Topic.ID) async -> Tab.ID? {
        // The size is a guess until the tab is shown and says its own.
        let opened = await send(machineID(of: topicID), .openTerminal(topicId: topicID, cols: 80, rows: 24))
        guard case .terminal(_, let terminalID) = opened else { return nil }
        return terminalID
    }

    /// Closing a tab ends its agent or shell, as on the desktop.
    func close(_ tabID: Tab.ID, in topicID: Topic.ID) {
        guard let tab = topic(topicID)?.tabs.first(where: { $0.id == tabID }) else { return }
        switch tab {
        case .agent: fire(machineID(of: topicID), .closeSession(tabID, force: true))
        case .terminal: fire(machineID(of: topicID), .closeTerminal(tabID))
        }
    }

    /// A copy of the session, up to `afterSeq` when given. Nil when it failed.
    func fork(_ sessionID: Session.ID, after afterSeq: UInt64? = nil) async -> Tab.ID? {
        let forked = await send(machineID(ofSession: sessionID), .forkSession(sessionID, afterSeq: afterSeq))
        guard case .session(_, let id) = forked else { return nil }
        return id
    }

    func importConversation(_ conversation: Conversation, into topicID: Topic.ID) async -> Tab.ID? {
        let imported = await send(
            machineID(of: topicID),
            .importConversation(topicId: topicID, agent: conversation.agent, id: conversation.id)
        )
        guard case .session(_, let id) = imported else { return nil }
        return id
    }

    // MARK: Images

    /// Keeps a picked image to send, named by its SHA-256 as the daemon names it.
    func pick(_ data: Data, mime: String, width: Int, height: Int, number: UInt32) -> PromptImage {
        let id = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        picked[id] = (mime, data)
        if let image = UIImage(data: data) { imageCache.setObject(image, forKey: id as NSString) }
        return PromptImage(id: id, number: number, mime: mime, width: UInt32(width), height: UInt32(height))
    }

    /// Gives a device the picked images it doesn't have yet. Images that weren't picked here came
    /// from a device, which has them already.
    private func give(_ images: [PromptImage], to machine: String?) async -> Bool {
        for image in images {
            let key = "\(machine ?? "")/\(image.id)"
            guard let bytes = picked[image.id], !sentImages.contains(key) else { continue }
            guard case .imageAdded(let id) = await send(machine, .addImage(mime: bytes.mime, base64: bytes.data.base64EncodedString())) else {
                return false
            }
            guard id == image.id else {
                report("The device stored a different image than the one picked")
                return false
            }
            sentImages.insert(key)
        }
        return true
    }

    /// An image in a session, from here if it was picked here or fetched before, and otherwise
    /// from its device. Thumbnails take the device's small copy.
    func image(_ image: PromptImage, in topicID: Topic.ID?, thumbnail: Bool) async -> UIImage? {
        let full = image.id as NSString
        let small = "\(image.id)/thumbnail" as NSString
        if let cached = imageCache.object(forKey: full) ?? (thumbnail ? imageCache.object(forKey: small) : nil) {
            return cached
        }
        guard let engine, let machine = machineID(of: topicID),
              case .image(let data, _) = try? await engine.request(machine, .readImage(image.id, thumbnail: thumbnail)),
              let bytes = Data(base64Encoded: data), let loaded = UIImage(data: bytes) else { return nil }
        imageCache.setObject(loaded, forKey: thumbnail ? small : full)
        return loaded
    }

    // MARK: Terminals

    /// What a terminal tab talks to: its device's engine connection, used off the main thread.
    func terminalLink(_ topicID: Topic.ID) -> TerminalLink? {
        guard let engine, let machine = machineID(of: topicID) else { return nil }
        return TerminalLink(engine: engine, machine: machine)
    }

    // MARK: Topics

    /// Creates a topic, then starts its first agent if there is one. Nil when it failed.
    func createTopic(in workspaceID: Workspace.ID, name: String, inRepo: Bool, agent: AgentSetup?) async -> Topic.ID? {
        guard let workspace = workspace(workspaceID) else { return nil }
        let machine = device(workspace.deviceID)?.machineID
        let created = await send(machine, .createTopic(workspaceId: workspaceID, name: name.isEmpty ? nil : name, atRoot: inRepo))
        guard case .topic(_, let topicID) = created else { return nil }
        // A command's changes have arrived when it returns, so the new topic can be found.
        changed(devices: true)
        if let agent { _ = await startAgent(in: topicID, agent) }
        return topicID
    }

    func rename(_ topicID: Topic.ID, to name: String) {
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        fire(machineID(of: topicID), .renameTopic(topicID, to: name))
    }

    /// Marking read clears its unread sessions. Marking unread, when it has none, brings
    /// back the ones that finished.
    func setUnread(_ unread: Bool, for topicID: Topic.ID) {
        guard let topic = topic(topicID) else { return }
        let sessions = unread ? topic.sessions.filter { $0.status == .done } : topic.sessions.filter(\.unread)
        for session in sessions {
            fire(machineID(of: topicID), .setUnread(session.id, unread: unread))
        }
    }

    func setUnread(_ unread: Bool, session sessionID: Session.ID) {
        fire(machineID(ofSession: sessionID), .setUnread(sessionID, unread: unread))
    }

    func markRead(_ sessionID: Session.ID) {
        guard session(sessionID)?.unread == true else { return }
        setUnread(false, session: sessionID)
    }

    func markAllRead() {
        for topic in workspaces.flatMap(\.topics) where isOnline(topic.id) {
            setUnread(false, for: topic.id)
        }
    }

    /// Archives a topic. When that would lose work and isn't forced, nothing happens and what
    /// it would lose comes back instead.
    func archive(_ topicID: Topic.ID, force: Bool) async -> ArchiveRisk? {
        guard case .archiveRisk(let risk) = await send(machineID(of: topicID), .archiveTopic(topicID, force: force)) else {
            return nil
        }
        return ArchiveRisk(risk)
    }

    func loadArchived() async {
        guard let engine else { return }
        var topics: [ArchivedTopic] = []
        for device in devices where device.online {
            guard let machine = device.machineID,
                  case .archivedTopics(let found) = try? await engine.request(machine, .listArchivedTopics) else { continue }
            topics += found.map {
                ArchivedTopic(id: $0.id, workspaceID: $0.workspaceId, name: $0.name, archived: ago($0.archivedAtMs))
            }
        }
        archived = topics
    }

    func restore(_ topic: ArchivedTopic) async -> Bool {
        let machine = device(workspace(topic.workspaceID)?.deviceID)?.machineID
        guard await send(machine, .restoreTopic(topic.id)) != nil else { return false }
        archived.removeAll { $0.id == topic.id }
        return true
    }

    /// Moves a topic to another folder. When leaving would lose work and isn't forced,
    /// nothing happens and what it would lose comes back instead.
    func move(_ topicID: Topic.ID, to target: MoveTarget, force: Bool) async -> ArchiveRisk? {
        let wire: Wire.MoveTarget = switch target {
        case .newWorktree: .newWorktree
        case .root: .root
        case .worktree(let id): .worktree(id)
        }
        guard case .archiveRisk(let risk) = await send(machineID(of: topicID), .moveTopic(topicID, to: wire, force: force)) else {
            return nil
        }
        return ArchiveRisk(risk)
    }

    /// Turns a device-wide server on or off for one topic. Matching the server's own setting
    /// clears the topic's, so it follows the device again.
    func setMcp(_ server: McpServer, on: Bool, for topicID: Topic.ID) {
        fire(machineID(of: topicID), .setTopicMcp(topicID, server: server.id, on: on == server.enabled ? nil : on))
    }

    func loadConversations(on deviceID: Device.ID?) async {
        guard let engine, let machine = device(deviceID)?.machineID else { return }
        do {
            guard case .conversations(let found) = try await engine.request(machine, .listConversations) else { return }
            conversations = found.map {
                Conversation(id: $0.id, agent: $0.agent, title: $0.title, cwd: $0.cwd, updated: ago($0.updatedAtMs))
            }
        } catch {
            report(error.localizedDescription)
        }
    }

    // MARK: Workspaces and devices

    func addWorkspace(on deviceID: Device.ID, path: String) async -> Bool {
        await send(device(deviceID)?.machineID, .addWorkspace(path: path)) != nil
    }

    func loadFolders(at path: String, on deviceID: Device.ID?) async {
        guard case .folders(let found) = await send(device(deviceID)?.machineID, .listFolders(path: path)) else { return }
        folders[Self.folderKey(path, deviceID)] = found.map { Folder(name: $0.name, path: $0.path, isRepo: $0.isRepo) }
    }

    func rename(device deviceID: Device.ID, to name: String) {
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name != device(deviceID)?.name else { return }
        fire(device(deviceID)?.machineID, .renameMachine(name))
    }

    func setKeepAwake(_ on: Bool, on deviceID: Device.ID) {
        fire(device(deviceID)?.machineID, .setDeviceSettings(Wire.DeviceSettings(keepAwake: on)))
    }

    func detectProviders(on deviceID: Device.ID) async {
        guard case .providerCandidates(let found) = await send(device(deviceID)?.machineID, .detectProviders) else { return }
        candidates[deviceID] = found.map { ProviderCandidate(agent: $0.agent, name: $0.name, missing: $0.missing) }
    }

    func addProvider(_ candidate: ProviderCandidate, on deviceID: Device.ID) async {
        guard await send(device(deviceID)?.machineID, .addProvider(candidate.agent)) != nil else { return }
        candidates[deviceID]?.removeAll { $0.agent == candidate.agent }
    }

    func removeProvider(_ agent: String, on deviceID: Device.ID) {
        fire(device(deviceID)?.machineID, .removeProvider(agent))
    }

    func setDefaults(model: String?, effort: String?, for agent: String, on deviceID: Device.ID) {
        fire(device(deviceID)?.machineID, .setProviderDefaults(agent, model: model, effort: effort))
    }

    func save(_ server: McpServer, isNew: Bool, on deviceID: Device.ID) {
        fire(device(deviceID)?.machineID, .saveMcpServer(
            id: isNew ? nil : server.id, name: server.name, topicId: server.topicID,
            transport: server.transport.wire, enabled: server.enabled
        ))
    }

    /// A placeholder for a server being added, which its device names once it's saved.
    func newServerID() -> McpServer.ID { "new-\(UUID().uuidString)" }

    func remove(server serverID: McpServer.ID, on deviceID: Device.ID) {
        fire(device(deviceID)?.machineID, .removeMcpServer(serverID))
    }

    /// The address to log in at, in a browser.
    func login(server serverID: McpServer.ID, on deviceID: Device.ID) async -> URL? {
        guard case .mcpLogin(let url) = await send(device(deviceID)?.machineID, .loginMcpServer(serverID)) else { return nil }
        return URL(string: url)
    }

    func completeLogin(state: String, code: String, on deviceID: Device.ID) async -> Bool {
        await send(device(deviceID)?.machineID, .completeMcpLogin(state: state, code: code)) != nil
    }

    func logout(server serverID: McpServer.ID, on deviceID: Device.ID) {
        fire(device(deviceID)?.machineID, .logoutMcpServer(serverID))
    }

    /// What it imported, worded.
    func importMcpServers(on deviceID: Device.ID) async -> String? {
        guard case .mcpImport(let added, let skipped) = await send(device(deviceID)?.machineID, .importMcpServers) else {
            return nil
        }
        let imported = added.count == 1 ? "Imported 1 server" : "Imported \(added.count) servers"
        return skipped.isEmpty ? imported : "\(imported), skipped \(skipped.joined(separator: ", "))"
    }

    func refreshConnectors(on deviceID: Device.ID) async -> String? {
        guard case .mcpConnectors(let added, let removed, let problems) = await send(device(deviceID)?.machineID, .refreshMcpConnectors) else {
            return nil
        }
        if let problem = problems.first { return problem }
        return "\(added.count) added, \(removed.count) removed"
    }

    func forget(_ clientID: PairedClient.ID, on deviceID: Device.ID) {
        fire(device(deviceID)?.machineID, .removePairedClient(key: clientID))
    }

    /// Pairs with the daemon a code names, and keeps it. Throws with a message to show.
    func pair(code: String) async throws {
        guard let engine else { return }
        try await engine.pair(code: code, name: UIDevice.current.name)
        changed(devices: true)
    }

    /// Stops reaching a device, taking this phone off its allowed list first while it can.
    func forget(device deviceID: Device.ID) async {
        guard let engine else { return }
        do {
            try await engine.forget(url: deviceID)
        } catch {
            report(error.localizedDescription)
        }
        changed(devices: true)
    }

    // MARK: Themes

    private struct ThemeFile: Decodable {
        let id: String
        let name: String
        let scheme: String
        let colors: [String: String]
    }

    private static func loadThemes() -> [Theme] {
        guard let url = Bundle.main.url(forResource: "themes", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let files = try? JSONDecoder().decode([ThemeFile].self, from: data)
        else { return [.tectrDark, .tectrLight] }
        return files.compactMap { Theme(id: $0.id, name: $0.name, scheme: $0.scheme == "light" ? .light : .dark, colors: $0.colors) }
    }
}

/// A terminal's way to its device, which works off the main thread.
struct TerminalLink: Sendable {
    let engine: Engine
    let machine: String
}

/// A session whose events come in.
private struct Following {
    /// Kept here because the session, and so the way to its machine, can go first.
    let machine: String
    var fold = TranscriptFold()
    let opened = ContinuousClock.now
    /// Whether its transcript is published, which waits for its history to arrive.
    var shown = false
    var settling: Task<Void, Never>?
}

/// What a new agent starts with.
struct AgentSetup: Hashable {
    var agent: String
    var model: String?
    var effort: String?
    var mode: PermissionMode = .auto
    var prompt = ""
    var images: [PromptImage] = []

    /// The device's defaults for its first provider.
    init?(device: Device?) {
        guard let provider = device?.providers.first else { return nil }
        self.init(provider: provider)
    }

    init(provider: Provider) {
        agent = provider.agent
        model = provider.defaultModel
        effort = provider.defaultEffort ?? provider.model(provider.defaultModel).flatMap { model in
            // A middle level when the provider has no default, as on the desktop.
            model.efforts.isEmpty ? nil : model.efforts[model.efforts.count / 2].id
        }
        if let mode = provider.modes.first(where: { $0 == .auto }) ?? provider.modes.first { self.mode = mode }
    }
}

// MARK: From the wire

private extension Provider {
    init(_ wire: Wire.Provider) {
        self.init(
            agent: wire.agent, name: wire.name, version: wire.version,
            modes: wire.modes.compactMap { PermissionMode(rawValue: $0.rawValue) },
            models: wire.models.map { model in
                ProviderModel(id: model.id, name: model.name, efforts: model.efforts.map { ProviderEffort(id: $0.id, name: $0.name) })
            },
            defaultModel: wire.defaultModel, defaultEffort: wire.defaultEffort, canFork: wire.canFork
        )
    }
}

private extension McpServer {
    init(_ wire: Wire.McpServer) {
        let transport: Transport = switch wire.transport {
        case .stdio(let command, let args, let env): .stdio(command: command, args: args, env: env)
        case .http(let url, let headers, let client): .http(url: url, headers: headers, oauthClientID: client)
        case .claudeConnector(let url): .claudeConnector(url: url)
        case .codexApp(let id): .codexApp(id: id)
        case .unknown: .stdio(command: "", args: [], env: [:])
        }
        let auth: Auth = switch wire.auth {
        case .needsLogin: .needsLogin
        case .loggedIn: .loggedIn
        case .none, .unknown: .none
        }
        self.init(id: wire.id, name: wire.name, topicID: wire.topicId, transport: transport, enabled: wire.enabled, auth: auth)
    }
}

private extension McpServer.Transport {
    var wire: Wire.McpTransport {
        switch self {
        case .stdio(let command, let args, let env): .stdio(command: command, args: args, env: env)
        case .http(let url, let headers, let client): .http(url: url, headers: headers, oauthClientId: client)
        case .claudeConnector(let url): .claudeConnector(url: url)
        case .codexApp(let id): .codexApp(id: id)
        }
    }
}

private extension Request {
    init(_ approval: Wire.Approval) {
        let primary = approval.options.first { $0.kind == .allowOnce } ?? approval.options.first
        self.init(
            id: approval.id,
            sessionID: approval.sessionId,
            kind: Self.kind(approval.toolCall.kind),
            command: approval.toolCall.title ?? "Untitled tool call",
            reason: approval.toolCall.locations?.joined(separator: "\n"),
            options: approval.options.map { option in
                let role: RequestCard.Option.Role = switch option.kind {
                case .rejectOnce, .rejectAlways: .reject
                default: option.id == primary?.id ? .primary : .secondary
                }
                return RequestCard.Option(id: option.id, name: option.name, role: role)
            }
        )
    }

    static func kind(_ kind: String?) -> String {
        switch kind {
        case "execute": "Run a command"
        case "edit": "Edit a file"
        case "delete": "Delete a file"
        case "move": "Move a file"
        case "read": "Read a file"
        case "fetch": "Fetch a page"
        default: "Allow a tool"
        }
    }
}

private extension ArchiveRisk {
    init(_ wire: Wire.ArchiveRisk) {
        self.init(
            running: Int(wire.running),
            worktrees: wire.worktrees.map {
                WorktreeRisk(path: $0.path, branch: $0.branch, uncommitted: Int($0.uncommitted), unpushed: Int($0.unpushed))
            }
        )
    }
}
