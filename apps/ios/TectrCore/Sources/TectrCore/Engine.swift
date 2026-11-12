import Foundation

/// A `protocol::Command`: its name and its fields, sent as `{"command": name, ...fields}`.
public struct Command: Encodable, Sendable {
    let name: String
    let fields: [String: any Encodable & Sendable]

    init(_ name: String, _ fields: [String: any Encodable & Sendable] = [:]) {
        self.name = name
        self.fields = fields
    }

    private struct Key: CodingKey {
        let stringValue: String
        var intValue: Int? { nil }
        init(_ string: String) { stringValue = string }
        init?(stringValue: String) { self.stringValue = stringValue }
        init?(intValue: Int) { nil }
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: Key.self)
        try c.encode(name, forKey: Key("command"))
        for (key, value) in fields {
            try c.encode(value, forKey: Key(key))
        }
    }
}

public extension Wire {
    /// Where a topic moves to.
    enum MoveTarget: Sendable {
        case newWorktree, root
        case worktree(String)
    }
}

private struct MoveTargetValue: Encodable, Sendable {
    let target: Wire.MoveTarget

    enum CodingKeys: String, CodingKey {
        case kind
        case worktreeId = "worktree_id"
    }

    func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch target {
        case .newWorktree: try c.encode("new_worktree", forKey: .kind)
        case .root: try c.encode("root", forKey: .kind)
        case .worktree(let id):
            try c.encode("worktree", forKey: .kind)
            try c.encode(id, forKey: .worktreeId)
        }
    }
}

public extension Command {
    static func addWorkspace(path: String) -> Command { Command("add_workspace", ["path": path]) }
    static func listFolders(path: String) -> Command { Command("list_folders", ["path": path]) }

    static func createTopic(workspaceId: String, name: String?, atRoot: Bool) -> Command {
        Command("create_topic", ["workspace_id": workspaceId, "name": name, "at_root": atRoot])
    }

    static func renameTopic(_ topicId: String, to name: String) -> Command {
        Command("rename_topic", ["topic_id": topicId, "name": name])
    }

    static func archiveTopic(_ topicId: String, force: Bool) -> Command {
        Command("archive_topic", ["topic_id": topicId, "force": force])
    }

    static func restoreTopic(_ topicId: String) -> Command { Command("restore_topic", ["topic_id": topicId]) }
    static let listArchivedTopics = Command("list_archived_topics")

    static func moveTopic(_ topicId: String, to target: Wire.MoveTarget, force: Bool) -> Command {
        Command("move_topic", ["topic_id": topicId, "to": MoveTargetValue(target: target), "force": force])
    }

    static func startSession(topicId: String, agent: String, mode: String, model: String?, effort: String?) -> Command {
        Command("start_session", ["topic_id": topicId, "agent": agent, "mode": mode, "model": model, "effort": effort])
    }

    static func sendPrompt(_ sessionId: String, text: String, images: [Wire.PromptImage]) -> Command {
        Command("send_prompt", ["session_id": sessionId, "text": text, "images": images])
    }

    static func addImage(mime: String, base64 data: String) -> Command { Command("add_image", ["mime": mime, "data": data]) }

    static func readImage(_ id: String, thumbnail: Bool) -> Command {
        Command("read_image", ["id": id, "thumbnail": thumbnail])
    }

    static func cancelTurn(_ sessionId: String) -> Command { Command("cancel_turn", ["session_id": sessionId]) }

    static func answerApproval(_ approvalId: String, option: String) -> Command {
        Command("answer_approval", ["approval_id": approvalId, "option_id": option])
    }

    static func renameMachine(_ name: String) -> Command { Command("rename_machine", ["name": name]) }
    static let detectProviders = Command("detect_providers")
    static func addProvider(_ agent: String) -> Command { Command("add_provider", ["agent": agent]) }
    static func removeProvider(_ agent: String) -> Command { Command("remove_provider", ["agent": agent]) }

    static func setProviderDefaults(_ agent: String, model: String?, effort: String?) -> Command {
        Command("set_provider_defaults", ["agent": agent, "model": model, "effort": effort])
    }

    static func setDeviceSettings(_ settings: Wire.DeviceSettings) -> Command {
        Command("set_device_settings", ["settings": settings])
    }

    static func setSessionSettings(_ sessionId: String, mode: String, model: String?, effort: String?) -> Command {
        Command("set_session_settings", ["session_id": sessionId, "mode": mode, "model": model, "effort": effort])
    }

    static func setUnread(_ sessionId: String, unread: Bool) -> Command {
        Command("set_unread", ["session_id": sessionId, "unread": unread])
    }

    static func setDraft(_ sessionId: String, text: String, images: [Wire.PromptImage]) -> Command {
        Command("set_draft", ["session_id": sessionId, "text": text, "images": images])
    }

    static func openTerminal(topicId: String, cols: UInt16, rows: UInt16) -> Command {
        Command("open_terminal", ["topic_id": topicId, "cols": cols, "rows": rows])
    }

    static func closeTerminal(_ terminalId: String) -> Command { Command("close_terminal", ["terminal_id": terminalId]) }

    static func closeSession(_ sessionId: String, force: Bool) -> Command {
        Command("close_session", ["session_id": sessionId, "force": force])
    }

    static func saveMcpServer(id: String?, name: String, topicId: String?, transport: Wire.McpTransport, enabled: Bool) -> Command {
        var fields: [String: any Encodable & Sendable] = ["name": name, "transport": transport, "enabled": enabled]
        if let id { fields["id"] = id }
        if let topicId { fields["topic_id"] = topicId }
        return Command("save_mcp_server", fields)
    }

    static func removeMcpServer(_ id: String) -> Command { Command("remove_mcp_server", ["id": id]) }

    static func setTopicMcp(_ topicId: String, server: String, on: Bool?) -> Command {
        Command("set_topic_mcp", ["topic_id": topicId, "server_id": server, "on": on])
    }

    static func loginMcpServer(_ id: String) -> Command { Command("login_mcp_server", ["id": id]) }

    static func completeMcpLogin(state: String, code: String) -> Command {
        Command("complete_mcp_login", ["state": state, "code": code])
    }

    static func logoutMcpServer(_ id: String) -> Command { Command("logout_mcp_server", ["id": id]) }
    static let importMcpServers = Command("import_mcp_servers")
    static let refreshMcpConnectors = Command("refresh_mcp_connectors")
    static let listConversations = Command("list_conversations")

    static func importConversation(topicId: String, agent: String, id: String) -> Command {
        Command("import_conversation", ["topic_id": topicId, "agent": agent, "id": id])
    }

    static func forkSession(_ sessionId: String, afterSeq: UInt64?) -> Command {
        var fields: [String: any Encodable & Sendable] = ["session_id": sessionId]
        if let afterSeq { fields["after_seq"] = afterSeq }
        return Command("fork_session", fields)
    }

    static func removePairedClient(key: String) -> Command { Command("remove_paired_client", ["key": key]) }
}

/// The engine, for the app: daemon connections, merged state and actions, decoded. It's safe
/// to call from any thread.
public final class Engine: Sendable {
    private let core: Core

    /// Keeps this phone's key and paired devices in `folder`, and starts connecting to them.
    public init(folder: URL, clientName: String) throws {
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        core = try plain { try Core(dataDir: folder.path, clientName: clientName) }
    }

    /// Calls `changed` after changes, at most once a frame, off the main thread, with whether
    /// the device tree changed. Otherwise only open sessions have new events.
    public func listen(_ changed: @escaping @Sendable (Bool) -> Void) {
        core.listen(listener: Forward(changed))
    }

    public func devices() -> [Wire.Device] {
        decode([Wire.Device].self, core.devices()) ?? []
    }

    public func request(_ machineId: String, _ command: Command) async throws -> Wire.CommandOutput {
        let json = String(decoding: try JSONEncoder().encode(command), as: UTF8.self)
        let output = try await plain { try await core.request(machineId: machineId, command: json) }
        guard let decoded = decode(Wire.CommandOutput.self, output) else {
            throw EngineFailure("the device sent something this app can't read")
        }
        return decoded
    }

    public func reconnect() { core.reconnect() }

    public func openSession(_ machineId: String, _ sessionId: String) {
        core.openSession(machineId: machineId, sessionId: sessionId)
    }

    public func closeSession(_ machineId: String, _ sessionId: String) {
        core.closeSession(machineId: machineId, sessionId: sessionId)
    }

    public func events(_ machineId: String, _ sessionId: String, after seq: UInt64) -> [Wire.SessionEvent] {
        decode([Wire.SessionEvent].self, core.eventsAfter(machineId: machineId, sessionId: sessionId, afterSeq: seq)) ?? []
    }

    /// Whether an open session's history has all arrived, so it can be shown in one piece.
    public func isReplayed(_ machineId: String, _ sessionId: String) -> Bool {
        core.isReplayed(machineId: machineId, sessionId: sessionId)
    }

    /// Starts showing a terminal. `frame` gets its frames off the main thread: kind 1 is output
    /// and kind 2 a reset, whose payload starts with the size. See `crates/protocol/src/frame.rs`.
    public func attachTerminal(
        _ machineId: String, _ terminalId: String, cols: UInt16, rows: UInt16,
        colors: Wire.TerminalColors?, frame: @escaping @Sendable (UInt8, Data) -> Void
    ) {
        let colors = colors.flatMap { try? JSONEncoder().encode($0) }.map { String(decoding: $0, as: UTF8.self) }
        core.attachTerminal(machineId: machineId, terminalId: terminalId, cols: cols, rows: rows, colors: colors, sink: Sink(frame))
    }

    public func detachTerminal(_ machineId: String, _ terminalId: String) {
        core.detachTerminal(machineId: machineId, terminalId: terminalId)
    }

    public func resizeTerminal(_ machineId: String, _ terminalId: String, cols: UInt16, rows: UInt16) {
        core.resizeTerminal(machineId: machineId, terminalId: terminalId, cols: cols, rows: rows)
    }

    public func terminalInput(_ machineId: String, _ terminalId: String, _ data: Data) throws {
        try plain { try core.terminalInput(machineId: machineId, terminalId: terminalId, data: data) }
    }

    public func terminalAck(_ machineId: String, _ terminalId: String, bytes: UInt32) {
        core.terminalAck(machineId: machineId, terminalId: terminalId, bytes: bytes)
    }

    /// Asks the device that showed `code` to pair. The answer has the digits to type there.
    public func pair(code: String, name: String) async throws -> PendingPairing {
        PendingPairing(try await plain { try await core.pair(code: code, name: name) })
    }

    /// Waits for the pairing's digits to be typed on the device, then keeps the device.
    public func finishPairing(_ pairing: PendingPairing) async throws {
        try await plain { try await core.finishPairing(pairing: pairing.inner) }
    }

    public func forget(url: String) async throws {
        try await plain { try await core.forget(url: url) }
    }

    /// Why a prompt can't run, when it's an agent command for something tectr sets itself.
    public static func refusal(of prompt: String) -> String? {
        refusedPrompt(prompt: prompt)
    }

    public func clientKey() throws -> String {
        try plain { try core.clientKey() }
    }

    private func decode<T: Decodable>(_ type: T.Type, _ json: String) -> T? {
        do {
            return try JSONDecoder().decode(type, from: Data(json.utf8))
        } catch {
            assertionFailure("couldn't decode \(type): \(error)")
            return nil
        }
    }
}

/// The engine's own errors carry a message written to be shown as is.
private func plain<T>(_ body: () throws -> T) throws -> T {
    do {
        return try body()
    } catch CoreError.Failed(let message) {
        throw EngineFailure(message)
    }
}

private func plain<T>(_ body: () async throws -> T) async throws -> T {
    do {
        return try await body()
    } catch CoreError.Failed(let message) {
        throw EngineFailure(message)
    }
}

/// A pairing whose digits someone types on the device. Stopping it, or letting it go, takes
/// this phone off the device's waiting list.
public final class PendingPairing: Sendable {
    fileprivate let inner: Pairing
    public let digits: String
    /// The device's name, where the digits are typed.
    public let machineName: String

    fileprivate init(_ inner: Pairing) {
        self.inner = inner
        digits = inner.digits()
        machineName = inner.machineName()
    }

    public func stop() {
        inner.stop()
    }
}

public struct EngineFailure: LocalizedError {
    let message: String

    init(_ message: String) {
        self.message = message
    }

    public var errorDescription: String? { message }
}

private final class Forward: Listener {
    let onChange: @Sendable (Bool) -> Void

    init(_ changed: @escaping @Sendable (Bool) -> Void) {
        onChange = changed
    }

    func changed(devices: Bool) {
        onChange(devices)
    }
}

private final class Sink: TerminalSink {
    let deliver: @Sendable (UInt8, Data) -> Void

    init(_ deliver: @escaping @Sendable (UInt8, Data) -> Void) {
        self.deliver = deliver
    }

    func frame(kind: UInt8, payload: Data) {
        deliver(kind, payload)
    }
}
