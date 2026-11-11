import Foundation
import TectrCore
import TectrDesign

// What the screens show, made by the store from what the engine reports. Times are already
// worded, so the views only lay them out.

typealias PromptImage = Wire.PromptImage

struct Device: Identifiable, Hashable {
    /// The daemon's URL, which it has before its first connection says who it is.
    let id: String
    /// Nil until it has connected once.
    var machineID: String?
    var name: String
    var online: Bool
    /// Why it can't be reached, or that it has stopped answering, in the words of whatever said so.
    var problem: String?
    var lastSeen: String?
    var keepAwake = true
    var providers: [Provider] = []
    var mcpServers: [McpServer] = []
    var pairedClients: [PairedClient] = []
}

struct Workspace: Identifiable, Hashable {
    let id: String
    let deviceID: Device.ID
    var name: String
    var path: String
    var topics: [Topic]
}

struct Topic: Identifiable, Hashable {
    let id: String
    var name: String
    var worktreeID: Worktree.ID
    var worktrees: [Worktree]
    /// Agents and terminals together, in the order they were opened.
    var tabs: [Tab]
    /// The device's MCP servers this topic turns on or off, against their own `enabled`.
    var mcp: [McpServer.ID: Bool] = [:]

    var worktree: Worktree? { worktrees.first { $0.id == worktreeID } }

    var sessions: [Session] {
        tabs.compactMap { if case .agent(let session) = $0 { session } else { nil } }
    }

    /// The most urgent of its sessions.
    var glyph: StatusGlyph {
        sessions.map(\.glyph).min { $0.urgency < $1.urgency } ?? .idle
    }

    var unreadCount: Int { sessions.filter(\.unread).count }
}

struct Worktree: Identifiable, Hashable {
    let id: String
    var path: String
    var branch: String
    /// At the repo's own path rather than a worktree of it.
    var isRepo = false
}

enum Tab: Identifiable, Hashable {
    case agent(Session)
    case terminal(Terminal)

    var id: String {
        switch self {
        case .agent(let session): session.id
        case .terminal(let terminal): terminal.id
        }
    }

    var title: String {
        switch self {
        case .agent(let session): session.title
        case .terminal(let terminal): terminal.title
        }
    }
}

enum SessionStatus: Hashable {
    case starting, ready, working, waiting, done, failed, stopped

    init(_ status: Wire.SessionStatus) {
        switch status {
        case .starting: self = .starting
        case .working: self = .working
        case .waitingForYou: self = .waiting
        case .done: self = .done
        case .failed: self = .failed
        case .stopped: self = .stopped
        case .ready, .unknown: self = .ready
        }
    }
}

enum PermissionMode: String, CaseIterable, Identifiable, Hashable {
    case manual, auto, yolo

    var id: Self { self }

    var label: String {
        switch self {
        case .manual: "Manual"
        case .auto: "Auto"
        case .yolo: "Yolo"
        }
    }
}

struct Session: Identifiable, Hashable {
    let id: String
    var title: String
    /// The provider's agent ID, such as `claude`.
    var agent: String
    var model: String?
    var effort: String?
    var mode: PermissionMode = .auto
    var status: SessionStatus
    var unread = false
    var draft = ""
    var draftImages: [PromptImage] = []
    var commands: [SlashCommand] = []

    var glyph: StatusGlyph {
        switch status {
        case .waiting: .waiting
        case .failed: .failed
        case .working, .starting: .working
        case .done: unread ? .done : .read
        // An agent that stopped after finishing still has something to read.
        case .stopped: unread ? .done : .idle
        case .ready: .idle
        }
    }

    var isRunning: Bool { status == .working || status == .starting || status == .waiting }
}

struct SlashCommand: Identifiable, Hashable {
    var id: String { name }
    let name: String
    let detail: String
}

enum TranscriptItem: Identifiable, Hashable {
    case prompt(id: String, text: String, images: [PromptImage])
    /// `forkAfter` is where a branch from this answer starts, when the agent said where it ends.
    case message(id: String, blocks: [MessageBlock], forkAfter: UInt64?)
    case activity(Activity)
    /// How a turn stopped, if not the usual way, or an error.
    case note(id: String, text: String, failed: Bool)

    var id: String {
        switch self {
        case .prompt(let id, _, _), .message(let id, _, _), .note(let id, _, _): id
        case .activity(let activity): activity.id
        }
    }
}

struct Activity: Identifiable, Hashable {
    let id: String
    var state: ActivityState
    var steps: [Step]
}

struct Step: Identifiable, Hashable {
    let id: String
    var kind: String
    var title: String
    var status: ToolCallRow.Status
    var diff: [DiffLine] = []
    var output: String?
}

struct Terminal: Identifiable, Hashable {
    enum Status: Hashable { case running, stopped, exited }

    let id: String
    var title: String
    var status: Status
}

/// Something an agent needs from you.
struct Request: Identifiable, Hashable {
    let id: String
    let sessionID: Session.ID
    var kind: String
    var command: String?
    var reason: String?
    var options: [RequestCard.Option]
}

struct Provider: Identifiable, Hashable {
    var id: String { agent }
    let agent: String
    var name: String
    var version: String?
    var modes: [PermissionMode]
    var models: [ProviderModel]
    var defaultModel: String?
    var defaultEffort: String?
    var canFork = false

    func model(_ id: String?) -> ProviderModel? { models.first { $0.id == id } }
}

struct ProviderModel: Identifiable, Hashable {
    let id: String
    var name: String
    var efforts: [ProviderEffort]
}

struct ProviderEffort: Identifiable, Hashable {
    let id: String
    var name: String
}

struct ProviderCandidate: Identifiable, Hashable {
    var id: String { agent }
    let agent: String
    var name: String
    /// Why it can't be added on this device.
    var missing: String?
}

struct McpServer: Identifiable, Hashable {
    enum Transport: Hashable {
        case stdio(command: String, args: [String], env: [String: String])
        case http(url: String, headers: [String: String], oauthClientID: String?)
        case claudeConnector(url: String)
        case codexApp(id: String)

        var isConnector: Bool {
            switch self {
            case .claudeConnector, .codexApp: true
            case .stdio, .http: false
            }
        }

        var summary: String {
            switch self {
            case .stdio(let command, let args, _): ([command] + args).joined(separator: " ")
            case .http(let url, _, _), .claudeConnector(let url): url
            case .codexApp(let id): id
            }
        }
    }

    enum Auth: Hashable { case none, needsLogin, loggedIn }

    let id: String
    var name: String
    /// The topic that has this server to itself. Nil is a device-wide server.
    var topicID: Topic.ID?
    var transport: Transport
    var enabled: Bool
    var auth: Auth = .none
}

struct PairedClient: Identifiable, Hashable {
    let id: String
    var name: String
    var paired: String
}

struct Conversation: Identifiable, Hashable {
    let id: String
    var agent: String
    var title: String
    var cwd: String
    var updated: String
}

struct ArchivedTopic: Identifiable, Hashable {
    let id: String
    let workspaceID: Workspace.ID
    var name: String
    var archived: String
}

struct Folder: Identifiable, Hashable {
    var id: String { path }
    var name: String
    var path: String
    var isRepo: Bool
}

/// What archiving a topic would lose.
struct ArchiveRisk: Hashable {
    var running: Int
    var worktrees: [WorktreeRisk]

    var isEmpty: Bool { running == 0 && worktrees.isEmpty }

    /// What would be thrown away in git, one line each, as the desktop words it.
    var losses: [String] {
        worktrees.flatMap { worktree in
            var lines: [String] = []
            if worktree.uncommitted > 0 {
                lines.append("\(counted(worktree.uncommitted, "uncommitted file")) in \(worktree.path)")
            }
            if worktree.unpushed > 0 {
                lines.append("\(counted(worktree.unpushed, "commit")) on \(worktree.branch) that no other branch or remote has")
            }
            return lines
        }
    }

    /// The first thing archiving asks, which is all it asks when nothing in git would be lost.
    var summary: String {
        let stops = running > 0 ? "Stops \(counted(running, "running agent")). " : ""
        return losses.isEmpty ? "\(stops)Its tabs come back when it's restored." : "\(stops)Its worktree has work that would be lost."
    }
}

private func counted(_ count: Int, _ thing: String) -> String {
    "\(count) \(thing)\(count == 1 ? "" : "s")"
}

struct WorktreeRisk: Hashable {
    var path: String
    var branch: String
    var uncommitted: Int
    var unpushed: Int
}

/// Where a topic can move.
enum MoveTarget: Hashable {
    case newWorktree
    case root
    case worktree(Worktree.ID)
}
