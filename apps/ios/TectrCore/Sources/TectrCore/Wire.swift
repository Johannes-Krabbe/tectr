import Foundation

/// What the engine sends and takes, written to match `crates/protocol` and `engine::Device`,
/// as the desktop's `engine.ts` is. Values a newer daemon adds decode as `unknown`, so an
/// older app skips them instead of failing.
public enum Wire {
    // MARK: Devices

    public struct Device: Decodable, Sendable {
        public let url: String
        public let connection: Connection
        public let state: DaemonState?
        /// Milliseconds since the Unix epoch.
        public let lastSeenMs: Int64?

        enum CodingKeys: String, CodingKey {
            case url, connection, state
            case lastSeenMs = "last_seen_ms"
        }
    }

    public enum Connection: Decodable, Sendable, Equatable {
        case connecting
        case online(stalled: Bool)
        case offline(reason: String)
        case incompatible(daemonMin: UInt32, daemonMax: UInt32)
        case unknown

        enum CodingKeys: String, CodingKey {
            case state, stalled, reason
            case daemonMin = "daemon_min"
            case daemonMax = "daemon_max"
        }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            switch try c.decode(String.self, forKey: .state) {
            case "connecting": self = .connecting
            case "online": self = .online(stalled: try c.decodeIfPresent(Bool.self, forKey: .stalled) ?? false)
            case "offline": self = .offline(reason: try c.decodeIfPresent(String.self, forKey: .reason) ?? "")
            case "incompatible":
                self = .incompatible(
                    daemonMin: try c.decode(UInt32.self, forKey: .daemonMin),
                    daemonMax: try c.decode(UInt32.self, forKey: .daemonMax)
                )
            default: self = .unknown
            }
        }

        /// Whether commands can be sent, which a stalled connection still takes.
        public var isOnline: Bool {
            if case .online = self { true } else { false }
        }
    }

    public struct Machine: Decodable, Sendable {
        public let id: String
        public let name: String
    }

    public struct DaemonState: Decodable, Sendable {
        public let machine: Machine
        public let workspaces: [Workspace]
        public let approvals: [Approval]
        public let providers: [Provider]
        public let settings: DeviceSettings
        public let drafts: [String: String]
        public let draftImages: [String: [PromptImage]]
        public let commands: [String: [AgentCommand]]
        public let mcpServers: [McpServer]
        public let pairedClients: [PairedClient]

        enum CodingKeys: String, CodingKey {
            case machine, workspaces, approvals, providers, settings, drafts, commands
            case draftImages = "draft_images"
            case mcpServers = "mcp_servers"
            case pairedClients = "paired_clients"
        }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            machine = try c.decode(Machine.self, forKey: .machine)
            workspaces = try c.decode([Workspace].self, forKey: .workspaces)
            approvals = try c.decode([Approval].self, forKey: .approvals)
            providers = try c.decodeIfPresent([Provider].self, forKey: .providers) ?? []
            settings = try c.decodeIfPresent(DeviceSettings.self, forKey: .settings) ?? DeviceSettings()
            drafts = try c.decodeIfPresent([String: String].self, forKey: .drafts) ?? [:]
            draftImages = try c.decodeIfPresent([String: [PromptImage]].self, forKey: .draftImages) ?? [:]
            commands = try c.decodeIfPresent([String: [AgentCommand]].self, forKey: .commands) ?? [:]
            mcpServers = try c.decodeIfPresent([McpServer].self, forKey: .mcpServers) ?? []
            pairedClients = try c.decodeIfPresent([PairedClient].self, forKey: .pairedClients) ?? []
        }
    }

    public struct DeviceSettings: Codable, Sendable {
        public var keepAwake: Bool

        public init(keepAwake: Bool = true) {
            self.keepAwake = keepAwake
        }

        enum CodingKeys: String, CodingKey {
            case keepAwake = "keep_awake"
        }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            keepAwake = try c.decodeIfPresent(Bool.self, forKey: .keepAwake) ?? true
        }
    }

    public struct PairedClient: Decodable, Sendable {
        public let key: String
        public let name: String
        public let pairedAtMs: Int64

        enum CodingKeys: String, CodingKey {
            case key, name
            case pairedAtMs = "paired_at_ms"
        }
    }

    // MARK: The tree

    public struct Workspace: Decodable, Sendable {
        public let id: String
        public let name: String
        public let path: String
        public let topics: [Topic]
    }

    public struct Topic: Decodable, Sendable {
        public let id: String
        public let name: String
        public let worktreeId: String
        public let worktrees: [Worktree]
        public let sessions: [Session]
        public let terminals: [Terminal]
        public let mcp: [String: Bool]

        enum CodingKeys: String, CodingKey {
            case id, name, worktrees, sessions, terminals, mcp
            case worktreeId = "worktree_id"
        }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            id = try c.decode(String.self, forKey: .id)
            name = try c.decode(String.self, forKey: .name)
            worktreeId = try c.decode(String.self, forKey: .worktreeId)
            worktrees = try c.decode([Worktree].self, forKey: .worktrees)
            sessions = try c.decode([Session].self, forKey: .sessions)
            terminals = try c.decodeIfPresent([Terminal].self, forKey: .terminals) ?? []
            mcp = try c.decodeIfPresent([String: Bool].self, forKey: .mcp) ?? [:]
        }
    }

    public struct Worktree: Decodable, Sendable {
        public let id: String
        public let path: String
        public let branch: String
    }

    public struct Session: Decodable, Sendable {
        public let id: String
        public let agent: String
        public let mode: PermissionMode
        public let model: String?
        public let effort: String?
        public let status: SessionStatus
        public let title: String?
        public let unread: Bool
        public let openedAtMs: Int64

        enum CodingKeys: String, CodingKey {
            case id, agent, mode, model, effort, status, title, unread
            case openedAtMs = "opened_at_ms"
        }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            id = try c.decode(String.self, forKey: .id)
            agent = try c.decode(String.self, forKey: .agent)
            mode = try c.decode(PermissionMode.self, forKey: .mode)
            model = try c.decodeIfPresent(String.self, forKey: .model)
            effort = try c.decodeIfPresent(String.self, forKey: .effort)
            status = try c.decode(SessionStatus.self, forKey: .status)
            title = try c.decodeIfPresent(String.self, forKey: .title)
            unread = try c.decodeIfPresent(Bool.self, forKey: .unread) ?? false
            openedAtMs = try c.decodeIfPresent(Int64.self, forKey: .openedAtMs) ?? 0
        }
    }

    public enum PermissionMode: String, Codable, Sendable {
        case manual, auto, yolo, unknown

        public init(from decoder: any Decoder) throws {
            self = Self(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .unknown
        }
    }

    public enum SessionStatus: String, Decodable, Sendable {
        case starting, ready, working, done, failed, stopped, unknown
        case waitingForYou = "waiting_for_you"

        public init(from decoder: any Decoder) throws {
            self = Self(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .unknown
        }
    }

    public struct Terminal: Decodable, Sendable {
        public let id: String
        public let title: String?
        public let status: TerminalStatus
        public let openedAtMs: Int64

        enum CodingKeys: String, CodingKey {
            case id, title, status
            case openedAtMs = "opened_at_ms"
        }
    }

    public enum TerminalStatus: String, Decodable, Sendable {
        case running, stopped, exited, unknown

        public init(from decoder: any Decoder) throws {
            self = Self(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .unknown
        }
    }

    /// A client's colors as `#rrggbb`, which programs ask a terminal for.
    public struct TerminalColors: Encodable, Sendable {
        public let foreground: String
        public let background: String
        public let cursor: String
        /// The 16 ANSI colors, black to bright white.
        public let palette: [String]

        public init(foreground: String, background: String, cursor: String, palette: [String]) {
            self.foreground = foreground
            self.background = background
            self.cursor = cursor
            self.palette = palette
        }
    }

    public struct AgentCommand: Decodable, Sendable {
        public let name: String
        public let description: String
        public let hint: String?
    }

    // MARK: Requests

    public struct Approval: Decodable, Sendable {
        public let id: String
        public let sessionId: String
        public let toolCall: ToolCallPatch
        public let options: [PermissionOption]

        enum CodingKeys: String, CodingKey {
            case id, options
            case sessionId = "session_id"
            case toolCall = "tool_call"
        }
    }

    public struct PermissionOption: Decodable, Sendable {
        public let id: String
        public let name: String
        public let kind: Kind

        public enum Kind: String, Decodable, Sendable {
            case allowOnce = "allow_once"
            case allowAlways = "allow_always"
            case rejectOnce = "reject_once"
            case rejectAlways = "reject_always"
            case unknown

            public init(from decoder: any Decoder) throws {
                self = Self(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .unknown
            }
        }
    }

    /// The fields of a tool call an event sets. A missing field keeps its previous value.
    public struct ToolCallPatch: Decodable, Sendable {
        public var title: String?
        public var kind: String?
        public var status: ToolCallStatus?
        public var input: JSONValue?
        public var locations: [String]?
        public var content: [ToolCallContent]?

        public init() {}

        /// This patch laid over `older`.
        public func applied(over older: ToolCallPatch) -> ToolCallPatch {
            var merged = older
            if let title { merged.title = title }
            if let kind { merged.kind = kind }
            if let status { merged.status = status }
            if let input { merged.input = input }
            if let locations { merged.locations = locations }
            if let content { merged.content = content }
            return merged
        }
    }

    public enum ToolCallStatus: String, Decodable, Sendable {
        case pending, completed, failed, unknown
        case inProgress = "in_progress"

        public init(from decoder: any Decoder) throws {
            self = Self(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .unknown
        }
    }

    public enum ToolCallContent: Decodable, Sendable {
        case text(String)
        case diff(path: String, oldText: String?, newText: String)
        case unknown

        enum CodingKeys: String, CodingKey {
            case type, text, path
            case oldText = "old_text"
            case newText = "new_text"
        }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            switch try c.decode(String.self, forKey: .type) {
            case "text": self = .text(try c.decode(String.self, forKey: .text))
            case "diff":
                self = .diff(
                    path: try c.decode(String.self, forKey: .path),
                    oldText: try c.decodeIfPresent(String.self, forKey: .oldText),
                    newText: try c.decode(String.self, forKey: .newText)
                )
            default: self = .unknown
            }
        }
    }

    // MARK: Providers and MCP

    public struct Provider: Decodable, Sendable {
        public let agent: String
        public let name: String
        public let version: String?
        public let modes: [PermissionMode]
        public let models: [ProviderModel]
        public let defaultModel: String?
        public let defaultEffort: String?
        public let recommendedModel: String?
        public let canFork: Bool

        enum CodingKeys: String, CodingKey {
            case agent, name, version, modes, models
            case defaultModel = "default_model"
            case defaultEffort = "default_effort"
            case recommendedModel = "recommended_model"
            case canFork = "can_fork"
        }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            agent = try c.decode(String.self, forKey: .agent)
            name = try c.decode(String.self, forKey: .name)
            version = try c.decodeIfPresent(String.self, forKey: .version)
            modes = try c.decode([PermissionMode].self, forKey: .modes)
            models = try c.decode([ProviderModel].self, forKey: .models)
            defaultModel = try c.decodeIfPresent(String.self, forKey: .defaultModel)
            defaultEffort = try c.decodeIfPresent(String.self, forKey: .defaultEffort)
            recommendedModel = try c.decodeIfPresent(String.self, forKey: .recommendedModel)
            canFork = try c.decodeIfPresent(Bool.self, forKey: .canFork) ?? false
        }
    }

    public struct ProviderModel: Decodable, Sendable {
        public let id: String
        public let name: String
        public let efforts: [ProviderEffort]
    }

    public struct ProviderEffort: Decodable, Sendable {
        public let id: String
        public let name: String
    }

    public struct ProviderCandidate: Decodable, Sendable {
        public let agent: String
        public let name: String
        public let missing: String?
    }

    public struct McpServer: Decodable, Sendable {
        public let id: String
        public let name: String
        public let topicId: String?
        public let transport: McpTransport
        public let enabled: Bool
        public let auth: McpAuth

        enum CodingKeys: String, CodingKey {
            case id, name, transport, enabled, auth
            case topicId = "topic_id"
        }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            id = try c.decode(String.self, forKey: .id)
            name = try c.decode(String.self, forKey: .name)
            topicId = try c.decodeIfPresent(String.self, forKey: .topicId)
            transport = try c.decode(McpTransport.self, forKey: .transport)
            enabled = try c.decode(Bool.self, forKey: .enabled)
            auth = try c.decodeIfPresent(McpAuth.self, forKey: .auth) ?? .none
        }
    }

    public enum McpTransport: Codable, Sendable {
        case stdio(command: String, args: [String], env: [String: String])
        case http(url: String, headers: [String: String], oauthClientId: String?)
        case claudeConnector(url: String)
        case codexApp(id: String)
        case unknown

        enum CodingKeys: String, CodingKey {
            case type, command, args, env, url, headers, id
            case oauthClientId = "oauth_client_id"
        }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            switch try c.decode(String.self, forKey: .type) {
            case "stdio":
                self = .stdio(
                    command: try c.decode(String.self, forKey: .command),
                    args: try c.decodeIfPresent([String].self, forKey: .args) ?? [],
                    env: try c.decodeIfPresent([String: String].self, forKey: .env) ?? [:]
                )
            case "http":
                self = .http(
                    url: try c.decode(String.self, forKey: .url),
                    headers: try c.decodeIfPresent([String: String].self, forKey: .headers) ?? [:],
                    oauthClientId: try c.decodeIfPresent(String.self, forKey: .oauthClientId)
                )
            case "claude_connector": self = .claudeConnector(url: try c.decode(String.self, forKey: .url))
            case "codex_app": self = .codexApp(id: try c.decode(String.self, forKey: .id))
            default: self = .unknown
            }
        }

        public func encode(to encoder: any Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            switch self {
            case .stdio(let command, let args, let env):
                try c.encode("stdio", forKey: .type)
                try c.encode(command, forKey: .command)
                try c.encode(args, forKey: .args)
                try c.encode(env, forKey: .env)
            case .http(let url, let headers, let oauthClientId):
                try c.encode("http", forKey: .type)
                try c.encode(url, forKey: .url)
                try c.encode(headers, forKey: .headers)
                try c.encodeIfPresent(oauthClientId, forKey: .oauthClientId)
            case .claudeConnector(let url):
                try c.encode("claude_connector", forKey: .type)
                try c.encode(url, forKey: .url)
            case .codexApp(let id):
                try c.encode("codex_app", forKey: .type)
                try c.encode(id, forKey: .id)
            case .unknown:
                throw EncodingError.invalidValue(self, .init(codingPath: [], debugDescription: "an unknown transport can't be sent back"))
            }
        }
    }

    public enum McpAuth: String, Decodable, Sendable {
        case none, unknown
        case needsLogin = "needs_login"
        case loggedIn = "logged_in"

        public init(from decoder: any Decoder) throws {
            self = Self(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .unknown
        }
    }

    // MARK: Images

    /// An image in a prompt or a draft. Its bytes stay with the daemon.
    public struct PromptImage: Codable, Sendable, Hashable {
        /// The SHA-256 of its bytes, so the same image has the same ID on every device.
        public let id: String
        /// What the text calls it: `[Image #3]` is number 3.
        public let number: UInt32
        public let mime: String
        public let width: UInt32
        public let height: UInt32

        public init(id: String, number: UInt32, mime: String, width: UInt32, height: UInt32) {
            self.id = id
            self.number = number
            self.mime = mime
            self.width = width
            self.height = height
        }

        public var token: String { "[Image #\(number)]" }
    }

    // MARK: Session events

    public struct SessionEvent: Decodable, Sendable {
        public let seq: UInt64
        public let atMs: Int64
        public let kind: SessionEventKind

        enum CodingKeys: String, CodingKey {
            case seq, kind
            case atMs = "at_ms"
        }
    }

    public enum SessionEventKind: Decodable, Sendable {
        case statusChanged(SessionStatus)
        case agentReady
        case userPrompt(text: String, images: [PromptImage])
        case agentMessage(String)
        case agentThought(String)
        case toolCall(id: String, tool: ToolCallPatch)
        case toolCallUpdate(id: String, tool: ToolCallPatch)
        case permissionRequested(Approval)
        /// The option chosen, or nil when the turn ended before anyone answered.
        case permissionResolved(approvalId: String, optionId: String?)
        case turnEnded(stopReason: String, lastMessage: String?)
        case error(String)
        case unknown

        enum CodingKeys: String, CodingKey {
            case event, status, text, images, id, tool, approval, outcome
            case approvalId = "approval_id"
            case stopReason = "stop_reason"
            case lastMessage = "last_message"
            case message
        }

        private struct Outcome: Decodable {
            let outcome: String
            let option_id: String?
        }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            switch try c.decode(String.self, forKey: .event) {
            case "status_changed": self = .statusChanged(try c.decode(SessionStatus.self, forKey: .status))
            case "agent_ready": self = .agentReady
            case "user_prompt":
                self = .userPrompt(
                    text: try c.decode(String.self, forKey: .text),
                    images: try c.decodeIfPresent([PromptImage].self, forKey: .images) ?? []
                )
            case "agent_message": self = .agentMessage(try c.decode(String.self, forKey: .text))
            case "agent_thought": self = .agentThought(try c.decode(String.self, forKey: .text))
            case "tool_call":
                self = .toolCall(id: try c.decode(String.self, forKey: .id), tool: try c.decode(ToolCallPatch.self, forKey: .tool))
            case "tool_call_update":
                self = .toolCallUpdate(id: try c.decode(String.self, forKey: .id), tool: try c.decode(ToolCallPatch.self, forKey: .tool))
            case "permission_requested": self = .permissionRequested(try c.decode(Approval.self, forKey: .approval))
            case "permission_resolved":
                let outcome = try c.decode(Outcome.self, forKey: .outcome)
                self = .permissionResolved(
                    approvalId: try c.decode(String.self, forKey: .approvalId),
                    optionId: outcome.outcome == "selected" ? outcome.option_id : nil
                )
            case "turn_ended":
                self = .turnEnded(
                    stopReason: try c.decode(String.self, forKey: .stopReason),
                    lastMessage: try c.decodeIfPresent(String.self, forKey: .lastMessage)
                )
            case "error": self = .error(try c.decode(String.self, forKey: .message))
            default: self = .unknown
            }
        }
    }

    // MARK: Command results

    public struct Folder: Decodable, Sendable {
        public let name: String
        public let path: String
        public let isRepo: Bool

        enum CodingKeys: String, CodingKey {
            case name, path
            case isRepo = "is_repo"
        }
    }

    public struct ArchiveRisk: Decodable, Sendable {
        public let running: UInt32
        public let worktrees: [WorktreeRisk]
    }

    public struct WorktreeRisk: Decodable, Sendable {
        public let path: String
        public let branch: String
        public let uncommitted: UInt32
        public let unpushed: UInt32
    }

    public struct ArchivedTopic: Decodable, Sendable {
        public let id: String
        public let workspaceId: String
        public let name: String
        public let archivedAtMs: Int64

        enum CodingKeys: String, CodingKey {
            case id, name
            case workspaceId = "workspace_id"
            case archivedAtMs = "archived_at_ms"
        }
    }

    public struct Conversation: Decodable, Sendable {
        public let agent: String
        public let id: String
        public let title: String
        public let cwd: String
        public let updatedAtMs: Int64

        enum CodingKeys: String, CodingKey {
            case agent, id, title, cwd
            case updatedAtMs = "updated_at_ms"
        }
    }

    /// What a command answers. Only the outputs the app reads have cases of their own.
    public enum CommandOutput: Decodable, Sendable {
        case topic(workspaceId: String, topicId: String)
        case session(topicId: String, sessionId: String)
        case terminal(topicId: String, terminalId: String)
        case providerCandidates([ProviderCandidate])
        case folders([Folder])
        case archiveRisk(ArchiveRisk)
        case archivedTopics([ArchivedTopic])
        case mcpLogin(url: String)
        case mcpImport(added: [String], skipped: [String])
        case mcpConnectors(added: [String], removed: [String], problems: [String])
        case conversations([Conversation])
        case imageAdded(id: String)
        case image(data: String, mime: String)
        case done
        case other

        enum CodingKeys: String, CodingKey {
            case output, topic, session, terminal, candidates, folders, risk, topics, url, added, skipped, removed
            case problems, conversations, id, data, mime
            case workspaceId = "workspace_id"
            case topicId = "topic_id"
        }

        private struct Identified: Decodable {
            let id: String
        }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            switch try c.decode(String.self, forKey: .output) {
            case "topic":
                self = .topic(
                    workspaceId: try c.decode(String.self, forKey: .workspaceId),
                    topicId: try c.decode(Identified.self, forKey: .topic).id
                )
            case "session":
                self = .session(
                    topicId: try c.decode(String.self, forKey: .topicId),
                    sessionId: try c.decode(Identified.self, forKey: .session).id
                )
            case "terminal":
                self = .terminal(
                    topicId: try c.decode(String.self, forKey: .topicId),
                    terminalId: try c.decode(Identified.self, forKey: .terminal).id
                )
            case "provider_candidates": self = .providerCandidates(try c.decode([ProviderCandidate].self, forKey: .candidates))
            case "folders": self = .folders(try c.decode([Folder].self, forKey: .folders))
            case "archive_risk": self = .archiveRisk(try c.decode(ArchiveRisk.self, forKey: .risk))
            case "archived_topics": self = .archivedTopics(try c.decode([ArchivedTopic].self, forKey: .topics))
            case "mcp_login": self = .mcpLogin(url: try c.decode(String.self, forKey: .url))
            case "mcp_import":
                self = .mcpImport(added: try c.decode([String].self, forKey: .added), skipped: try c.decode([String].self, forKey: .skipped))
            case "mcp_connectors":
                self = .mcpConnectors(
                    added: try c.decode([String].self, forKey: .added),
                    removed: try c.decode([String].self, forKey: .removed),
                    problems: try c.decode([String].self, forKey: .problems)
                )
            case "conversations": self = .conversations(try c.decode([Conversation].self, forKey: .conversations))
            case "image_added": self = .imageAdded(id: try c.decode(String.self, forKey: .id))
            case "image": self = .image(data: try c.decode(String.self, forKey: .data), mime: try c.decode(String.self, forKey: .mime))
            case "done": self = .done
            default: self = .other
            }
        }
    }
}

/// Any JSON value, for the parts of the protocol that pass one through, such as a tool's input.
public enum JSONValue: Codable, Sendable, Hashable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: any Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() {
            self = .null
        } else if let value = try? c.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? c.decode(Double.self) {
            self = .number(value)
        } else if let value = try? c.decode(String.self) {
            self = .string(value)
        } else if let value = try? c.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = .object(try c.decode([String: JSONValue].self))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let value): try c.encode(value)
        case .number(let value): try c.encode(value)
        case .string(let value): try c.encode(value)
        case .array(let value): try c.encode(value)
        case .object(let value): try c.encode(value)
        }
    }

    public subscript(key: String) -> JSONValue? {
        if case .object(let object) = self { object[key] } else { nil }
    }

    public var string: String? {
        if case .string(let value) = self { value } else { nil }
    }
}
