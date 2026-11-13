import TectrCore
import TectrDesign

// The same board as design/fixtures/board.json, with transcripts, settings and requests added
// so every screen has something real-looking to show.

extension Store {
    static func preview() -> Store {
        Store(
            devices: [laptop, desktop, vps],
            workspaces: [tectrLaptop, tectrDesktop, relay],
            requests: [
                Request(
                    id: "r1", sessionID: "s1", kind: "Run a command",
                    command: "rm -rf target/debug/incremental",
                    reason: "Clears stale incremental artifacts that make the reconnect tests fail to link.",
                    options: [
                        .init(id: "allow", name: "Allow", role: .primary),
                        .init(id: "always", name: "Always allow rm in this topic", role: .secondary),
                        .init(id: "reject", name: "Deny", role: .reject),
                    ]
                ),
                Request(
                    id: "r2", sessionID: "s3", kind: "Edit a file",
                    command: "crates/daemon/src/spawn.rs",
                    reason: "Passes the worktree path to the adapter as its working directory.",
                    options: [
                        .init(id: "allow", name: "Allow", role: .primary),
                        .init(id: "always", name: "Always allow edits", role: .secondary),
                        .init(id: "reject", name: "Deny", role: .reject),
                    ]
                ),
            ],
            archived: [
                ArchivedTopic(id: "a1", workspaceID: "tectr-laptop", name: "Pairing codes", archived: "3d"),
                ArchivedTopic(id: "a2", workspaceID: "tectr-desktop", name: "Token build script", archived: "1w"),
            ],
            conversations: [
                Conversation(id: "c1", agent: "claude", title: "Fix flaky iroh relay test", cwd: "~/Developer/tectr", updated: "2h"),
                Conversation(id: "c2", agent: "codex", title: "Explain the event resume path", cwd: "~/Developer/tectr", updated: "1d"),
                Conversation(id: "c3", agent: "claude", title: "Draft the release notes for v0.3", cwd: "~/Developer/tectr", updated: "4d"),
            ],
            candidates: ["laptop": [ProviderCandidate(agent: "gemini", name: "Gemini CLI", missing: "gemini isn't installed")]],
            folders: [
                "laptop\n~": [
                    Folder(name: "Developer", path: "~/Developer", isRepo: false),
                    Folder(name: "Documents", path: "~/Documents", isRepo: false),
                    Folder(name: "dotfiles", path: "~/dotfiles", isRepo: true),
                ],
                "laptop\n~/Developer": [
                    Folder(name: "tectr", path: "~/Developer/tectr", isRepo: true),
                    Folder(name: "twig", path: "~/Developer/twig", isRepo: true),
                    Folder(name: "website", path: "~/Developer/website", isRepo: true),
                    Folder(name: "scratch", path: "~/Developer/scratch", isRepo: false),
                ],
            ],
            transcripts: transcripts
        )
    }

    private static let transcripts: [Session.ID: [TranscriptItem]] = [
        "s1": backoffTranscript,
        "s2": [
            .prompt(id: "s2-p1", text: "Write a test matrix for reconnects: daemon restart, network drop, and resume after N.", images: []),
            .activity(Activity(id: "s2-a1", state: .running("Editing tests/reconnect.rs"), steps: [
                Step(id: "s2-a1-1", kind: "read", title: "crates/engine/tests/scripted_daemon.rs", status: .done),
                Step(id: "s2-a1-2", kind: "edit", title: "crates/engine/tests/reconnect.rs", status: .running),
            ])),
        ],
        "s3": [
            .prompt(id: "s3-p1", text: "Spawn each adapter in its topic's worktree.", images: []),
            .activity(Activity(id: "s3-a1", state: .waiting, steps: [
                Step(id: "s3-a1-1", kind: "search", title: "current_dir", status: .done),
                Step(id: "s3-a1-2", kind: "edit", title: "crates/daemon/src/spawn.rs", status: .running),
            ])),
        ],
        "s4": [
            .prompt(id: "s4-p1", text: "Parse markdown incrementally as tokens stream in.", images: []),
            .activity(Activity(id: "s4-a1", state: .finished("Worked for 2m 14s, 9 steps", failed: 1), steps: [
                Step(id: "s4-a1-1", kind: "edit", title: "apps/dsk/src/markdown.ts", status: .done),
                Step(id: "s4-a1-2", kind: "execute", title: "pnpm test markdown", status: .failed,
                     output: "3 failed, 41 passed\n  x keeps an open fence open\n  x closes a list on a blank line\n  x streams a heading"),
            ])),
            .message(id: "s4-m1", blocks: [
                .paragraph("Three tests fail: an open code fence closes too early when a chunk ends inside it."),
            ], forkAfter: nil),
        ],
        "s5": [
            .prompt(id: "s5-p1", text: "Only render the messages in view.", images: []),
            .activity(Activity(id: "s5-a1", state: .finished("Worked for 6m 3s, 14 steps", failed: 0), steps: [])),
            .message(id: "s5-m1", blocks: [
                .paragraph("The transcript now renders only what's in view, plus one screen above and below."),
                .bullets(["Heights are measured once and cached", "Scrolling to the end stays pinned while streaming"]),
            ], forkAfter: 9),
        ],
    ]

    private static let efforts = [
        ProviderEffort(id: "low", name: "Low"),
        ProviderEffort(id: "medium", name: "Medium"),
        ProviderEffort(id: "high", name: "High"),
    ]

    private static let claude = Provider(
        agent: "claude", name: "Claude Code", version: "2.1.4", modes: [.manual, .auto, .yolo],
        models: [
            ProviderModel(id: "opus", name: "Opus", efforts: efforts),
            ProviderModel(id: "sonnet", name: "Sonnet", efforts: efforts),
            ProviderModel(id: "haiku", name: "Haiku", efforts: []),
        ],
        defaultModel: "opus", defaultEffort: "high", canFork: true
    )

    private static let codex = Provider(
        agent: "codex", name: "Codex", version: "0.98.0", modes: [.manual, .auto, .yolo],
        models: [ProviderModel(id: "gpt-codex", name: "GPT Codex", efforts: efforts)],
        defaultModel: "gpt-codex", defaultEffort: "medium"
    )

    private static let servers: [McpServer] = [
        McpServer(id: "m1", name: "linear", transport: .http(url: "https://mcp.linear.app/mcp", headers: [:], oauthClientID: nil),
                  enabled: true, auth: .loggedIn),
        McpServer(id: "m2", name: "postgres", transport: .stdio(command: "npx", args: ["-y", "@modelcontextprotocol/server-postgres"],
                                                                env: ["DATABASE_URL": "${DATABASE_URL}"]), enabled: false),
        McpServer(id: "m3", name: "github", transport: .http(url: "https://api.githubcopilot.com/mcp", headers: [:], oauthClientID: nil),
                  enabled: true, auth: .needsLogin),
        McpServer(id: "m4", name: "claude.ai Slack", transport: .claudeConnector(url: "https://mcp.slack.com"), enabled: true),
        McpServer(id: "m5", name: "Figma", transport: .codexApp(id: "figma"), enabled: false),
    ]

    private static let laptop = Device(
        id: "laptop", name: "Laptop", online: true, providers: [claude, codex],
        mcpServers: servers,
        pairedClients: [PairedClient(id: "p1", name: "iPhone", paired: "3d"), PairedClient(id: "p2", name: "Desktop", paired: "2w")]
    )

    private static let desktop = Device(
        id: "desktop", name: "Desktop", online: true, providers: [claude, codex],
        mcpServers: Array(servers.prefix(2)),
        pairedClients: [PairedClient(id: "p3", name: "iPhone", paired: "3d"), PairedClient(id: "p4", name: "Laptop", paired: "2w")]
    )

    private static let vps = Device(id: "vps", name: "VPS", online: false, lastSeen: "2h ago", providers: [claude])

    private static let commands = [
        SlashCommand(name: "review", detail: "Review the changes on this branch"),
        SlashCommand(name: "compact", detail: "Summarize the conversation so far"),
        SlashCommand(name: "init", detail: "Write a CLAUDE.md for this repo"),
        SlashCommand(name: "usage", detail: "Show plan usage"),
    ]

    private static let tectrLaptop = Workspace(id: "tectr-laptop", deviceID: "laptop", name: "tectr", path: "~/Developer/tectr", topics: [
        Topic(
            id: "reconnect", name: "Client reconnect backoff", worktreeID: "reconnect-main",
            worktrees: [
                Worktree(id: "reconnect-main", path: "~/Developer/tectr/.worktrees/reconnect-backoff", branch: "reconnect-backoff"),
                Worktree(id: "reconnect-tests", path: "~/Developer/tectr/.worktrees/reconnect-tests", branch: "reconnect-tests"),
            ],
            tabs: [
                .agent(Session(
                    id: "s1", title: "Add capped backoff", agent: "claude", model: "opus", effort: "high",
                    status: .waiting, commands: commands
                )),
                .agent(Session(
                    id: "s2", title: "Reconnect test matrix", agent: "codex", model: "gpt-codex", effort: "medium",
                    status: .working
                )),
                .terminal(Terminal(id: "t1", title: "cargo test", status: .running)),
            ],
            mcp: ["m2": true]
        ),
        Topic(
            id: "spawn", name: "Spawn sessions via ACP", worktreeID: "spawn-main",
            worktrees: [Worktree(id: "spawn-main", path: "~/Developer/tectr/.worktrees/acp-spawn", branch: "acp-spawn")],
            tabs: [.agent(Session(
                id: "s3", title: "Spawn adapters", agent: "claude", model: "sonnet", effort: "medium",
                status: .waiting, commands: commands
            ))]
        ),
        Topic(
            id: "repo-chat", name: "Release planning", worktreeID: "root",
            worktrees: [Worktree(id: "root", path: "~/Developer/tectr", branch: "main", isRepo: true)],
            tabs: [.agent(Session(id: "s7", title: "What ships in v0.4", agent: "claude", model: "opus", status: .done))]
        ),
    ])

    private static let tectrDesktop = Workspace(id: "tectr-desktop", deviceID: "desktop", name: "tectr", path: "~/src/tectr", topics: [
        Topic(
            id: "markdown", name: "Incremental markdown parsing", worktreeID: "markdown-main",
            worktrees: [Worktree(id: "markdown-main", path: "~/src/tectr/.worktrees/incremental-markdown", branch: "incremental-markdown")],
            tabs: [.agent(Session(
                id: "s4", title: "Parser", agent: "claude", model: "opus", effort: "high", status: .failed
            ))]
        ),
        Topic(
            id: "transcript", name: "Virtualized transcript", worktreeID: "transcript-main",
            worktrees: [Worktree(id: "transcript-main", path: "~/src/tectr/.worktrees/virtual-transcript", branch: "virtual-transcript")],
            tabs: [.agent(Session(
                id: "s5", title: "Viewport", agent: "codex", model: "gpt-codex", status: .done, unread: true
            ))]
        ),
    ])

    private static let relay = Workspace(id: "relay-vps", deviceID: "vps", name: "push-relay", path: "~/code/push-relay", topics: [
        Topic(
            id: "apns", name: "APNs token refresh", worktreeID: "apns-main",
            worktrees: [Worktree(id: "apns-main", path: "~/code/push-relay/.worktrees/apns-refresh", branch: "apns-refresh")],
            tabs: [.agent(Session(id: "s6", title: "Token refresh", agent: "claude", model: "sonnet", status: .working))]
        ),
    ])

    private static let backoffTranscript: [TranscriptItem] = [
        .prompt(id: "s1-p1", text: "Cap the reconnect backoff at 30s and add jitter, so a fleet of phones doesn't reconnect in lockstep.", images: []),
        .activity(Activity(id: "s1-a1", state: .finished("Worked for 12s, 4 steps", failed: 0), steps: [
            Step(id: "s1-a1-1", kind: "read", title: "crates/engine/src/connection.rs", status: .done),
            Step(id: "s1-a1-2", kind: "search", title: "fn backoff", status: .done),
            Step(id: "s1-a1-3", kind: "edit", title: "crates/engine/src/connection.rs", status: .done, diff: [
                DiffLine(.context, "let delay = BASE * 2u32.pow(attempt.min(6));"),
                DiffLine(.removed, "sleep(delay).await;"),
                DiffLine(.added, "let jitter = rand::random::<u64>() % 250;"),
                DiffLine(.added, "sleep(delay.min(MAX_BACKOFF) + Duration::from_millis(jitter)).await;"),
            ]),
            Step(id: "s1-a1-4", kind: "think", title: "Thought for 3s", status: .done),
        ])),
        .message(id: "s1-m1", blocks: [
            .heading("Backoff"),
            .paragraph("The delay now stops at **30s**, and every wait adds up to `250ms` of jitter."),
            .bullets(["`MAX_BACKOFF` is a constant next to `BASE`", "The jitter uses the thread RNG, so tests seed it"]),
            .code(language: "rust", text: "const MAX_BACKOFF: Duration = Duration::from_secs(30);"),
            .paragraph("The reconnect tests fail to link because of stale incremental artifacts, so I'd like to clear them."),
        ], forkAfter: nil),
        .activity(Activity(id: "s1-a2", state: .waiting, steps: [
            Step(id: "s1-a2-1", kind: "execute", title: "cargo test -p engine reconnect", status: .failed,
                 output: "error: linking with `cc` failed: exit status: 1"),
            Step(id: "s1-a2-2", kind: "execute", title: "rm -rf target/debug/incremental", status: .running),
        ])),
    ]
}
