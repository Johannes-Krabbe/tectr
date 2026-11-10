//! SQLite persistence. Workspaces, topics, and session events survive a daemon
//! restart, and resuming a client's event stream reads from here.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use protocol::{
    AgentCommand, Approval, ApprovalId, ArchivedTopic, DaemonState, DeviceSettings, Machine,
    MachineId, McpServer, McpServerId, PairedClient, PermissionMode, Provider, Session,
    SessionEvent, SessionEventKind, SessionId, SessionRef, SessionStatus, Terminal, TerminalId,
    TerminalStatus, Topic, TopicId, Workspace, WorkspaceId, Worktree, WorktreeId,
};

use crate::oauth::Login;
use rusqlite::{Connection, OptionalExtension, params};

const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE workspaces (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        path TEXT NOT NULL UNIQUE
    );
    CREATE TABLE topics (
        id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL REFERENCES workspaces(id),
        name TEXT NOT NULL
    );
    CREATE TABLE worktrees (
        id TEXT PRIMARY KEY,
        topic_id TEXT NOT NULL REFERENCES topics(id),
        path TEXT NOT NULL,
        branch TEXT NOT NULL
    );
    CREATE TABLE sessions (
        id TEXT PRIMARY KEY,
        topic_id TEXT NOT NULL REFERENCES topics(id),
        worktree_id TEXT NOT NULL REFERENCES worktrees(id),
        agent TEXT NOT NULL,
        mode TEXT NOT NULL,
        model TEXT,
        status TEXT NOT NULL,
        title TEXT,
        agent_session_id TEXT
    );
    CREATE TABLE events (
        session_id TEXT NOT NULL REFERENCES sessions(id),
        seq INTEGER NOT NULL,
        at_ms INTEGER NOT NULL,
        kind TEXT NOT NULL,
        PRIMARY KEY (session_id, seq)
    ) WITHOUT ROWID;
    CREATE TABLE pending_approvals (
        id TEXT PRIMARY KEY,
        approval TEXT NOT NULL
    );
",
    "
    ALTER TABLE sessions ADD COLUMN effort TEXT;
    CREATE TABLE providers (
        agent TEXT PRIMARY KEY,
        provider TEXT NOT NULL
    );
",
    "
    ALTER TABLE sessions ADD COLUMN closed_at_ms INTEGER;
",
    // A number can only have come from the daemon, so those topics are the ones nobody named.
    // They catch up with their first tab here, because its title won't arrive again.
    "
    ALTER TABLE topics ADD COLUMN deleted_at_ms INTEGER;
    ALTER TABLE topics ADD COLUMN auto_named INTEGER NOT NULL DEFAULT 0;
    UPDATE topics SET auto_named = 1 WHERE name GLOB 'Topic #[0-9]*';
    UPDATE topics SET name = COALESCE(
        (SELECT title FROM sessions
         WHERE sessions.topic_id = topics.id AND closed_at_ms IS NULL
         ORDER BY rowid LIMIT 1),
        name
    ) WHERE auto_named = 1;
",
    "
    ALTER TABLE sessions ADD COLUMN unread INTEGER NOT NULL DEFAULT 0;
",
    // A topic's tabs share one folder now. SQLite can't drop a column a foreign key uses, so
    // `sessions.worktree_id` stays, and is kept equal to the topic's folder.
    "
    ALTER TABLE topics RENAME COLUMN deleted_at_ms TO archived_at_ms;
    ALTER TABLE topics ADD COLUMN worktree_id TEXT REFERENCES worktrees(id);
    ALTER TABLE sessions ADD COLUMN agent_cwd TEXT;
    UPDATE sessions SET agent_cwd = (SELECT path FROM worktrees WHERE id = sessions.worktree_id)
        WHERE agent_session_id IS NOT NULL;
    UPDATE topics SET worktree_id =
        (SELECT id FROM worktrees WHERE topic_id = topics.id ORDER BY rowid LIMIT 1);
    UPDATE sessions SET worktree_id =
        (SELECT worktree_id FROM topics WHERE id = sessions.topic_id);
",
    // Logins live apart from the servers, because servers go to every client and logins never
    // leave the daemon.
    "
    CREATE TABLE mcp_servers (id TEXT PRIMARY KEY, server TEXT NOT NULL);
    CREATE TABLE mcp_logins (server_id TEXT PRIMARY KEY, login TEXT NOT NULL);
    ALTER TABLE topics ADD COLUMN mcp TEXT;
",
    "
    ALTER TABLE sessions ADD COLUMN commands TEXT;
",
    // Tabs are in the order they were opened, agents and terminals together. A session from
    // before this was opened when its first event happened.
    "
    ALTER TABLE sessions ADD COLUMN opened_at_ms INTEGER NOT NULL DEFAULT 0;
    UPDATE sessions SET opened_at_ms =
        COALESCE((SELECT MIN(at_ms) FROM events WHERE events.session_id = sessions.id), 0);
    CREATE TABLE terminals (
        id TEXT PRIMARY KEY,
        topic_id TEXT NOT NULL REFERENCES topics(id),
        title TEXT,
        cwd TEXT,
        opened_at_ms INTEGER NOT NULL,
        closed_at_ms INTEGER
    );
",
    "
    CREATE TABLE paired_clients (
        key TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        paired_at_ms INTEGER NOT NULL
    );
",
];

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open database at {}", path.display()))?;
        Self::init(conn)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let applied: usize = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        for (index, migration) in MIGRATIONS.iter().enumerate().skip(applied) {
            let tx = conn.transaction()?;
            tx.execute_batch(migration)?;
            tx.pragma_update(None, "user_version", index + 1)?;
            tx.commit()?;
        }
        Ok(Self { conn })
    }

    /// Returns this machine's ID, generating it the first time.
    pub fn machine_id(&self) -> Result<MachineId> {
        let existing: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'machine_id'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(id) = existing {
            return Ok(MachineId(id));
        }
        let id = crate::new_id();
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('machine_id', ?1)",
            [&id],
        )?;
        Ok(MachineId(id))
    }

    /// The secret half of the key this daemon is reached by over iroh, made the first
    /// time something asks. It's what a paired client checks it's talking to.
    pub fn remote_key(&self, generate: impl FnOnce() -> String) -> Result<String> {
        let existing: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'remote_key'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(key) = existing {
            return Ok(key);
        }
        let key = generate();
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('remote_key', ?1)",
            [&key],
        )?;
        Ok(key)
    }

    pub fn insert_paired_client(&self, client: &PairedClient) -> Result<()> {
        self.conn.execute(
            "INSERT INTO paired_clients (key, name, paired_at_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT (key) DO UPDATE SET name = excluded.name,
                 paired_at_ms = excluded.paired_at_ms",
            params![client.key, client.name, client.paired_at_ms],
        )?;
        Ok(())
    }

    /// Whether there was one to remove.
    pub fn delete_paired_client(&self, key: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM paired_clients WHERE key = ?1", [key])?
            > 0)
    }

    /// The name someone gave this machine, if they did.
    pub fn machine_name(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'machine_name'",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// `None` forgets the name, so the machine goes back to its hostname.
    pub fn set_machine_name(&self, name: Option<&str>) -> Result<()> {
        match name {
            Some(name) => self.conn.execute(
                "INSERT INTO meta (key, value) VALUES ('machine_name', ?1)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                [name],
            )?,
            None => self
                .conn
                .execute("DELETE FROM meta WHERE key = 'machine_name'", [])?,
        };
        Ok(())
    }

    pub fn load_state(&self, machine: Machine) -> Result<DaemonState> {
        let mut workspaces = self.collect(
            "SELECT id, name, path FROM workspaces ORDER BY rowid",
            [],
            |row| {
                Ok(Workspace {
                    id: WorkspaceId(row.get(0)?),
                    name: row.get(1)?,
                    path: row.get(2)?,
                    topics: vec![],
                })
            },
        )?;
        for workspace in &mut workspaces {
            workspace.topics = self.collect(
                "SELECT id, name, worktree_id, mcp FROM topics
                 WHERE workspace_id = ?1 AND archived_at_ms IS NULL ORDER BY rowid",
                [&workspace.id.0],
                topic_row,
            )?;
            for topic in &mut workspace.topics {
                self.fill_topic(topic)?;
            }
        }
        let providers: Vec<String> =
            self.collect("SELECT provider FROM providers ORDER BY rowid", [], |row| {
                row.get(0)
            })?;
        let mcp_servers: Vec<String> =
            self.collect("SELECT server FROM mcp_servers ORDER BY rowid", [], |row| {
                row.get(0)
            })?;
        Ok(DaemonState {
            machine,
            workspaces,
            approvals: vec![],
            providers: providers
                .iter()
                .map(|provider| serde_json::from_str(provider))
                .collect::<Result<_, _>>()?,
            drafts: self.drafts()?,
            commands: self.commands(
                "SELECT sessions.id, sessions.commands FROM sessions
                 JOIN topics ON topics.id = sessions.topic_id
                 WHERE sessions.closed_at_ms IS NULL AND topics.archived_at_ms IS NULL
                     AND sessions.commands IS NOT NULL",
                [],
            )?,
            settings: self.device_settings()?,
            mcp_servers: mcp_servers
                .iter()
                .filter_map(|server| serde_json::from_str(server).ok())
                .collect(),
            paired_clients: self.collect(
                "SELECT key, name, paired_at_ms FROM paired_clients ORDER BY rowid",
                [],
                |row| {
                    Ok(PairedClient {
                        key: row.get(0)?,
                        name: row.get(1)?,
                        paired_at_ms: row.get(2)?,
                    })
                },
            )?,
        })
    }

    /// Like drafts, a value a newer version wrote that doesn't parse falls back to the defaults.
    fn device_settings(&self) -> Result<DeviceSettings> {
        let stored: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'settings'", [], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(stored
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default())
    }

    pub fn set_device_settings(&self, settings: &DeviceSettings) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('settings', ?1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [serde_json::to_string(settings)?],
        )?;
        Ok(())
    }

    fn fill_topic(&self, topic: &mut Topic) -> Result<()> {
        topic.worktrees = self.collect(
            "SELECT id, path, branch FROM worktrees WHERE topic_id = ?1 ORDER BY rowid",
            [&topic.id.0],
            |row| {
                Ok(Worktree {
                    id: WorktreeId(row.get(0)?),
                    path: row.get(1)?,
                    branch: row.get(2)?,
                })
            },
        )?;
        topic.sessions = self.collect(
            "SELECT id, agent, mode, model, status, title, effort, unread, opened_at_ms
             FROM sessions WHERE topic_id = ?1 AND closed_at_ms IS NULL ORDER BY rowid",
            [&topic.id.0],
            |row| {
                let mode: String = row.get(2)?;
                let status: String = row.get(4)?;
                Ok(Session {
                    id: SessionId(row.get(0)?),
                    agent: row.get(1)?,
                    mode: from_column::<PermissionMode>(&mode),
                    model: row.get(3)?,
                    effort: row.get(6)?,
                    status: from_column::<SessionStatus>(&status),
                    title: row.get(5)?,
                    unread: row.get(7)?,
                    opened_at_ms: row.get(8)?,
                })
            },
        )?;
        // No shell outlives the daemon, so each starts again when it's next shown.
        topic.terminals = self.collect(
            "SELECT id, title, opened_at_ms
             FROM terminals WHERE topic_id = ?1 AND closed_at_ms IS NULL ORDER BY rowid",
            [&topic.id.0],
            |row| {
                Ok(Terminal {
                    id: TerminalId(row.get(0)?),
                    title: row.get(1)?,
                    status: TerminalStatus::Stopped,
                    opened_at_ms: row.get(2)?,
                })
            },
        )?;
        Ok(())
    }

    /// An archived topic with its tabs and the workspace it's in.
    pub fn archived_topic(&self, id: &TopicId) -> Result<Option<(WorkspaceId, Topic)>> {
        let found = self
            .conn
            .query_row(
                "SELECT id, name, worktree_id, workspace_id, mcp FROM topics
                 WHERE id = ?1 AND archived_at_ms IS NOT NULL",
                [&id.0],
                |row| Ok((topic_row(row)?, WorkspaceId(row.get(3)?))),
            )
            .optional()?;
        let Some((mut topic, workspace_id)) = found else {
            return Ok(None);
        };
        self.fill_topic(&mut topic)?;
        Ok(Some((workspace_id, topic)))
    }

    /// Newest first.
    pub fn archived_topics(&self) -> Result<Vec<ArchivedTopic>> {
        self.collect(
            "SELECT id, workspace_id, name, archived_at_ms FROM topics
             WHERE archived_at_ms IS NOT NULL ORDER BY archived_at_ms DESC, rowid DESC",
            [],
            |row| {
                Ok(ArchivedTopic {
                    id: TopicId(row.get(0)?),
                    workspace_id: WorkspaceId(row.get(1)?),
                    name: row.get(2)?,
                    archived_at_ms: row.get(3)?,
                })
            },
        )
    }

    /// The branch each of a topic's worktrees was made with, which is the one tectr owns.
    /// What's checked out there now can differ.
    pub fn made_branches(&self, topic_id: &TopicId) -> Result<Vec<(WorktreeId, String)>> {
        self.collect(
            "SELECT id, branch FROM worktrees WHERE topic_id = ?1 ORDER BY rowid",
            [&topic_id.0],
            |row| Ok((WorktreeId(row.get(0)?), row.get(1)?)),
        )
    }

    /// A value written by a newer version might not parse. Losing drafts is better than
    /// not starting.
    fn drafts(&self) -> Result<BTreeMap<SessionId, String>> {
        let stored: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'drafts'", [], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(stored
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default())
    }

    /// The commands of the open tabs `sql` selects, as `(id, commands)`. Like drafts, a list a
    /// newer version wrote that doesn't parse is lost, and the agent sends it again anyway.
    fn commands(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<BTreeMap<SessionId, Vec<AgentCommand>>> {
        let rows: Vec<(String, String)> =
            self.collect(sql, params, |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, text)| {
                let mut commands: Vec<AgentCommand> = serde_json::from_str(&text).ok()?;
                // One stored before tectr took a command over still has it.
                commands.retain(|command| protocol::tectr_owned_command(&command.name).is_none());
                Some((SessionId(id), commands))
            })
            .collect())
    }

    /// The commands of an archived topic's open tabs, for restoring it.
    pub fn topic_commands(
        &self,
        topic_id: &TopicId,
    ) -> Result<BTreeMap<SessionId, Vec<AgentCommand>>> {
        self.commands(
            "SELECT id, commands FROM sessions
             WHERE topic_id = ?1 AND closed_at_ms IS NULL AND commands IS NOT NULL",
            [&topic_id.0],
        )
    }

    pub fn set_session_commands(&self, id: &SessionId, commands: &[AgentCommand]) -> Result<()> {
        let text = if commands.is_empty() {
            None
        } else {
            Some(serde_json::to_string(commands)?)
        };
        self.conn.execute(
            "UPDATE sessions SET commands = ?2 WHERE id = ?1",
            params![id.0, text],
        )?;
        Ok(())
    }

    pub fn set_drafts(&self, drafts: &BTreeMap<SessionId, String>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('drafts', ?1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [serde_json::to_string(drafts)?],
        )?;
        Ok(())
    }

    /// Takes a session out of the app. Its events stay, because they're the record of what
    /// the agent did.
    pub fn close_session(&self, id: &SessionId, at_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET closed_at_ms = ?2 WHERE id = ?1",
            params![id.0, at_ms],
        )?;
        Ok(())
    }

    pub fn insert_terminal(
        &self,
        topic_id: &TopicId,
        terminal: &Terminal,
        cwd: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO terminals (id, topic_id, title, cwd, opened_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                terminal.id.0,
                topic_id.0,
                terminal.title,
                cwd,
                terminal.opened_at_ms
            ],
        )?;
        Ok(())
    }

    pub fn set_terminal_title(&self, id: &TerminalId, title: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE terminals SET title = ?2 WHERE id = ?1",
            params![id.0, title],
        )?;
        Ok(())
    }

    /// Where its shell last was, which the next one starts in.
    pub fn terminal_cwd(&self, id: &TerminalId) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT cwd FROM terminals WHERE id = ?1", [&id.0], |row| {
                row.get(0)
            })
            .optional()?
            .flatten())
    }

    pub fn set_terminal_cwd(&self, id: &TerminalId, cwd: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE terminals SET cwd = ?2 WHERE id = ?1",
            params![id.0, cwd],
        )?;
        Ok(())
    }

    pub fn close_terminal(&self, id: &TerminalId, at_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE terminals SET closed_at_ms = ?2 WHERE id = ?1",
            params![id.0, at_ms],
        )?;
        Ok(())
    }

    fn collect<P: rusqlite::Params, T>(
        &self,
        sql: &str,
        params: P,
        map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>> {
        let mut statement = self.conn.prepare_cached(sql)?;
        let rows = statement.query_map(params, map)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn insert_workspace(&self, workspace: &Workspace) -> Result<()> {
        self.conn.execute(
            "INSERT INTO workspaces (id, name, path) VALUES (?1, ?2, ?3)",
            params![workspace.id.0, workspace.name, workspace.path],
        )?;
        Ok(())
    }

    /// `auto_named` says nobody chose this name, so the topic may go by its first tab's title.
    pub fn insert_topic(
        &self,
        workspace_id: &WorkspaceId,
        topic: &Topic,
        auto_named: bool,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO topics (id, workspace_id, name, auto_named) VALUES (?1, ?2, ?3, ?4)",
            params![topic.id.0, workspace_id.0, topic.name, auto_named],
        )?;
        for worktree in &topic.worktrees {
            tx.execute(
                "INSERT INTO worktrees (id, topic_id, path, branch) VALUES (?1, ?2, ?3, ?4)",
                params![worktree.id.0, topic.id.0, worktree.path, worktree.branch],
            )?;
        }
        // Set after its worktrees, because it points at one of them.
        tx.execute(
            "UPDATE topics SET worktree_id = ?2 WHERE id = ?1",
            params![topic.id.0, topic.worktree_id.0],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// `branch` is the one it's made with, which tectr owns unless it's the repo's own folder.
    pub fn insert_worktree(&self, topic_id: &TopicId, worktree: &Worktree) -> Result<()> {
        self.conn.execute(
            "INSERT INTO worktrees (id, topic_id, path, branch) VALUES (?1, ?2, ?3, ?4)",
            params![worktree.id.0, topic_id.0, worktree.path, worktree.branch],
        )?;
        Ok(())
    }

    /// Moves the topic and all its tabs to one of its worktrees.
    pub fn set_topic_worktree(&self, topic_id: &TopicId, worktree_id: &WorktreeId) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE topics SET worktree_id = ?2 WHERE id = ?1",
            params![topic_id.0, worktree_id.0],
        )?;
        tx.execute(
            "UPDATE sessions SET worktree_id = ?2 WHERE topic_id = ?1",
            params![topic_id.0, worktree_id.0],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// A name a person chose, which nothing changes on its own after this.
    pub fn rename_topic(&self, id: &TopicId, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE topics SET name = ?2, auto_named = 0 WHERE id = ?1",
            params![id.0, name],
        )?;
        Ok(())
    }

    /// Says whether the topic took the name, which it only does if nobody chose its name.
    pub fn auto_name_topic(&self, id: &TopicId, name: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE topics SET name = ?2 WHERE id = ?1 AND auto_named = 1",
            params![id.0, name],
        )?;
        Ok(changed > 0)
    }

    /// Takes a topic out of the app, keeping its tabs and their events. Only the folder its
    /// tabs work in is remembered, because that's the one restoring makes again.
    pub fn archive_topic(&self, id: &TopicId, at_ms: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE topics SET archived_at_ms = ?2 WHERE id = ?1",
            params![id.0, at_ms],
        )?;
        tx.execute(
            "DELETE FROM worktrees WHERE topic_id = ?1
             AND id IS NOT (SELECT worktree_id FROM topics WHERE id = ?1)",
            [&id.0],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn restore_topic(&self, id: &TopicId) -> Result<()> {
        self.conn.execute(
            "UPDATE topics SET archived_at_ms = NULL WHERE id = ?1",
            [&id.0],
        )?;
        Ok(())
    }

    pub fn insert_session(&self, topic_id: &TopicId, session: &Session) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions (id, topic_id, worktree_id, agent, mode, model, status, title, effort, unread, opened_at_ms)
             VALUES (?1, ?2, (SELECT worktree_id FROM topics WHERE id = ?2), ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                session.id.0,
                topic_id.0,
                session.agent,
                to_column(&session.mode),
                session.model,
                to_column(&session.status),
                session.title,
                session.effort,
                session.unread,
                session.opened_at_ms,
            ],
        )?;
        Ok(())
    }

    /// Keeps a provider's place in the list when it's refreshed.
    pub fn upsert_provider(&self, provider: &Provider) -> Result<()> {
        self.conn.execute(
            "INSERT INTO providers (agent, provider) VALUES (?1, ?2)
             ON CONFLICT (agent) DO UPDATE SET provider = excluded.provider",
            params![provider.agent, serde_json::to_string(provider)?],
        )?;
        Ok(())
    }

    /// Keeps a server's place in the list when it changes.
    pub fn upsert_mcp_server(&self, server: &McpServer) -> Result<()> {
        self.conn.execute(
            "INSERT INTO mcp_servers (id, server) VALUES (?1, ?2)
             ON CONFLICT (id) DO UPDATE SET server = excluded.server",
            params![server.id.0, serde_json::to_string(server)?],
        )?;
        Ok(())
    }

    /// Its login goes too.
    pub fn remove_mcp_server(&self, id: &McpServerId) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM mcp_servers WHERE id = ?1", [&id.0])?;
        tx.execute("DELETE FROM mcp_logins WHERE server_id = ?1", [&id.0])?;
        let topics: Vec<(String, String)> = {
            let mut statement = tx.prepare("SELECT id, mcp FROM topics WHERE mcp IS NOT NULL")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (topic, text) in topics {
            let mut mcp: BTreeMap<McpServerId, bool> =
                serde_json::from_str(&text).unwrap_or_default();
            if mcp.remove(id).is_some() {
                tx.execute(
                    "UPDATE topics SET mcp = ?2 WHERE id = ?1",
                    params![topic, serde_json::to_string(&mcp)?],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_topic_mcp(&self, id: &TopicId, mcp: &BTreeMap<McpServerId, bool>) -> Result<()> {
        self.conn.execute(
            "UPDATE topics SET mcp = ?2 WHERE id = ?1",
            params![id.0, serde_json::to_string(mcp)?],
        )?;
        Ok(())
    }

    pub fn mcp_login(&self, id: &McpServerId) -> Result<Option<Login>> {
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT login FROM mcp_logins WHERE server_id = ?1",
                [&id.0],
                |row| row.get(0),
            )
            .optional()?;
        Ok(stored.and_then(|text| serde_json::from_str(&text).ok()))
    }

    pub fn set_mcp_login(&self, id: &McpServerId, login: &Login) -> Result<()> {
        self.conn.execute(
            "INSERT INTO mcp_logins (server_id, login) VALUES (?1, ?2)
             ON CONFLICT (server_id) DO UPDATE SET login = excluded.login",
            params![id.0, serde_json::to_string(login)?],
        )?;
        Ok(())
    }

    pub fn remove_mcp_login(&self, id: &McpServerId) -> Result<()> {
        self.conn
            .execute("DELETE FROM mcp_logins WHERE server_id = ?1", [&id.0])?;
        Ok(())
    }

    pub fn remove_provider(&self, agent: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM providers WHERE agent = ?1", [agent])?;
        Ok(())
    }

    pub fn update_session(&self, session: &Session) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET status = ?2, title = ?3, mode = ?4, model = ?5, effort = ?6, unread = ?7
             WHERE id = ?1",
            params![
                session.id.0,
                to_column(&session.status),
                session.title,
                to_column(&session.mode),
                session.model,
                session.effort,
                session.unread,
            ],
        )?;
        Ok(())
    }

    /// The agent's own ID for this session, used to resume it after its
    /// process has gone away.
    pub fn agent_session_id(&self, session_id: &SessionId) -> Result<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT agent_session_id FROM sessions WHERE id = ?1",
            [&session_id.0],
            |row| row.get(0),
        )?)
    }

    pub fn set_agent_session_id(&self, session_id: &SessionId, agent_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET agent_session_id = ?2 WHERE id = ?1",
            params![session_id.0, agent_id],
        )?;
        Ok(())
    }

    /// The folder the agent last heard it was working in, so it can be told when that changes.
    pub fn agent_cwd(&self, session_id: &SessionId) -> Result<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT agent_cwd FROM sessions WHERE id = ?1",
            [&session_id.0],
            |row| row.get(0),
        )?)
    }

    pub fn set_agent_cwd(&self, session_id: &SessionId, cwd: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET agent_cwd = ?2 WHERE id = ?1",
            params![session_id.0, cwd],
        )?;
        Ok(())
    }

    pub fn insert_event(&self, event: &SessionEvent) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO events (session_id, seq, at_ms, kind) VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![
                event.session.session_id.0,
                event.seq,
                event.at_ms,
                serde_json::to_string(&event.kind)?,
            ])?;
        Ok(())
    }

    pub fn events_after(
        &self,
        machine_id: &MachineId,
        session_id: &SessionId,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<SessionEvent>> {
        let rows: Vec<(u64, i64, String)> = self.collect(
            "SELECT seq, at_ms, kind FROM events
             WHERE session_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
            params![session_id.0, after_seq, limit],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        rows.into_iter()
            .map(|(seq, at_ms, kind)| {
                Ok(SessionEvent {
                    session: SessionRef {
                        machine_id: machine_id.clone(),
                        session_id: session_id.clone(),
                    },
                    seq,
                    at_ms,
                    kind: serde_json::from_str::<SessionEventKind>(&kind)?,
                })
            })
            .collect()
    }

    pub fn last_seqs(&self) -> Result<Vec<(SessionId, u64)>> {
        self.collect(
            "SELECT session_id, MAX(seq) FROM events GROUP BY session_id",
            [],
            |row| Ok((SessionId(row.get(0)?), row.get(1)?)),
        )
    }

    pub fn insert_pending_approval(&self, approval: &Approval) -> Result<()> {
        self.conn.execute(
            "INSERT INTO pending_approvals (id, approval) VALUES (?1, ?2)",
            params![approval.id.0, serde_json::to_string(approval)?],
        )?;
        Ok(())
    }

    pub fn delete_pending_approval(&self, id: &ApprovalId) -> Result<()> {
        self.conn
            .execute("DELETE FROM pending_approvals WHERE id = ?1", [&id.0])?;
        Ok(())
    }

    pub fn pending_approvals(&self) -> Result<Vec<Approval>> {
        let rows: Vec<String> =
            self.collect("SELECT approval FROM pending_approvals", [], |row| {
                row.get(0)
            })?;
        rows.iter()
            .map(|row| Ok(serde_json::from_str(row)?))
            .collect()
    }
}

/// Every query using this selects `mcp` too, read by name because their other columns differ.
fn topic_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Topic> {
    let mcp: Option<String> = row.get("mcp")?;
    Ok(Topic {
        id: TopicId(row.get(0)?),
        name: row.get(1)?,
        worktree_id: WorktreeId(row.get(2)?),
        worktrees: vec![],
        sessions: vec![],
        terminals: vec![],
        // Like drafts, a value a newer version wrote that doesn't parse is lost.
        mcp: mcp
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default(),
    })
}

fn to_column<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        other => unreachable!("expected a unit enum, got {other:?}"),
    }
}

fn from_column<T: serde::de::DeserializeOwned>(text: &str) -> T {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .expect("every enum stored here has an Unknown variant")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::init(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn machine() -> Machine {
        Machine {
            id: "m".into(),
            name: "m".into(),
        }
    }

    #[test]
    fn machine_name_can_be_set_and_forgotten() {
        let store = store();
        assert_eq!(store.machine_name().unwrap(), None);
        store.set_machine_name(Some("Desk")).unwrap();
        store.set_machine_name(Some("Laptop")).unwrap();
        assert_eq!(store.machine_name().unwrap().as_deref(), Some("Laptop"));
        store.set_machine_name(None).unwrap();
        assert_eq!(store.machine_name().unwrap(), None);
    }

    #[test]
    fn paired_clients_and_the_remote_key_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tectr.db");
        let client = PairedClient {
            key: "k".into(),
            name: "Laptop".into(),
            paired_at_ms: 5,
        };
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.remote_key(|| "secret".into()).unwrap(), "secret");
            store.insert_paired_client(&client).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .remote_key(|| unreachable!("the key is kept"))
                .unwrap(),
            "secret"
        );
        let state = store.load_state(machine()).unwrap();
        assert_eq!(state.paired_clients, vec![client]);
        assert!(store.delete_paired_client("k").unwrap());
        assert!(!store.delete_paired_client("k").unwrap());
        assert!(
            store
                .load_state(machine())
                .unwrap()
                .paired_clients
                .is_empty()
        );
    }

    #[test]
    fn machine_id_is_stable() {
        let store = store();
        assert_eq!(store.machine_id().unwrap(), store.machine_id().unwrap());
    }

    #[test]
    fn state_round_trips_through_the_database() {
        let store = store();
        let session = Session {
            id: "s".into(),
            agent: "claude".into(),
            mode: PermissionMode::Auto,
            model: Some("haiku".into()),
            effort: None,
            status: SessionStatus::Done,
            title: None,
            unread: true,
            opened_at_ms: 1000,
        };
        let topic = Topic {
            id: "t".into(),
            name: "topic".into(),
            worktree_id: "wt".into(),
            worktrees: vec![Worktree {
                id: "wt".into(),
                path: "/wt".into(),
                branch: "tectr/topic".into(),
            }],
            sessions: vec![],
            terminals: vec![],
            mcp: BTreeMap::new(),
        };
        let workspace = Workspace {
            id: "w".into(),
            name: "repo".into(),
            path: "/repo".into(),
            topics: vec![],
        };
        store.insert_workspace(&workspace).unwrap();
        store.insert_topic(&workspace.id, &topic, false).unwrap();
        store.insert_session(&topic.id, &session).unwrap();

        let state = store.load_state(machine()).unwrap();
        assert_eq!(state.workspaces[0].topics[0].worktrees, topic.worktrees);
        assert_eq!(state.workspaces[0].topics[0].sessions, vec![session]);
    }

    #[test]
    fn a_closed_session_stays_gone_and_what_is_on_screen_is_kept() {
        let store = store();
        let workspace = Workspace {
            id: "w".into(),
            name: "repo".into(),
            path: "/repo".into(),
            topics: vec![],
        };
        let topic = Topic {
            id: "t".into(),
            name: "topic".into(),
            worktree_id: "wt".into(),
            worktrees: vec![Worktree {
                id: "wt".into(),
                path: "/wt".into(),
                branch: "b".into(),
            }],
            sessions: vec![],
            terminals: vec![],
            mcp: BTreeMap::new(),
        };
        store.insert_workspace(&workspace).unwrap();
        store.insert_topic(&workspace.id, &topic, false).unwrap();
        for id in ["s1", "s2"] {
            let session = Session {
                id: id.into(),
                agent: "a".into(),
                mode: PermissionMode::Auto,
                model: None,
                effort: None,
                status: SessionStatus::Done,
                title: None,
                unread: false,
                opened_at_ms: 0,
            };
            store.insert_session(&topic.id, &session).unwrap();
        }
        store.close_session(&"s1".into(), 1).unwrap();

        let mut drafts = BTreeMap::new();
        drafts.insert(SessionId::from("s2"), "half a".to_owned());
        store.set_drafts(&drafts).unwrap();
        drafts.insert(SessionId::from("s2"), "half a tho".to_owned());
        store.set_drafts(&drafts).unwrap();

        let command = |name: &str| AgentCommand {
            name: name.to_owned(),
            description: String::new(),
            hint: None,
        };
        // Stored before tectr took `model` over, and before s1 closed.
        for id in ["s1", "s2"] {
            store
                .set_session_commands(&id.into(), &[command("usage"), command("model")])
                .unwrap();
        }

        let state = store.load_state(machine()).unwrap();
        let sessions = &state.workspaces[0].topics[0].sessions;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, SessionId::from("s2"));
        assert_eq!(state.drafts, drafts);
        assert_eq!(
            state.commands,
            BTreeMap::from([(SessionId::from("s2"), vec![command("usage")])])
        );
    }

    fn stored_topic(store: &Store, id: &str, auto_named: bool) -> Topic {
        let worktree = Worktree {
            id: format!("{id}-wt").as_str().into(),
            path: format!("/wt/{id}"),
            branch: format!("tectr/{id}"),
        };
        let topic = Topic {
            id: id.into(),
            name: "Topic #1".into(),
            worktree_id: worktree.id.clone(),
            worktrees: vec![worktree],
            sessions: vec![],
            terminals: vec![],
            mcp: BTreeMap::new(),
        };
        store.insert_topic(&"w".into(), &topic, auto_named).unwrap();
        topic
    }

    fn with_workspace() -> Store {
        let store = store();
        store
            .insert_workspace(&Workspace {
                id: "w".into(),
                name: "repo".into(),
                path: "/repo".into(),
                topics: vec![],
            })
            .unwrap();
        store
    }

    #[test]
    fn a_chosen_name_is_not_replaced_and_survives_a_restart() {
        let store = with_workspace();
        stored_topic(&store, "auto", true);
        stored_topic(&store, "chosen", false);

        assert!(store.auto_name_topic(&"auto".into(), "From a tab").unwrap());
        assert!(
            !store
                .auto_name_topic(&"chosen".into(), "From a tab")
                .unwrap()
        );

        store.rename_topic(&"auto".into(), "Mine").unwrap();
        assert!(!store.auto_name_topic(&"auto".into(), "Later").unwrap());

        let state = store.load_state(machine()).unwrap();
        let names: Vec<_> = state.topics().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Mine", "Topic #1"]);
    }

    #[test]
    fn an_archived_topic_stays_gone_until_restored_with_only_its_folder() {
        let store = with_workspace();
        stored_topic(&store, "kept", false);
        let mut topic = stored_topic(&store, "gone", false);
        let left = Worktree {
            id: "left".into(),
            path: "/wt/left".into(),
            branch: "tectr/left".into(),
        };
        store.insert_worktree(&topic.id, &left).unwrap();
        store.set_topic_worktree(&topic.id, &left.id).unwrap();
        let session = Session {
            id: "s".into(),
            agent: "a".into(),
            mode: PermissionMode::Auto,
            model: None,
            effort: None,
            status: SessionStatus::Done,
            title: None,
            unread: false,
            opened_at_ms: 0,
        };
        store.insert_session(&topic.id, &session).unwrap();

        store.archive_topic(&topic.id, 7).unwrap();
        let state = store.load_state(machine()).unwrap();
        let ids: Vec<_> = state.topics().map(|t| t.id.0.as_str()).collect();
        assert_eq!(ids, ["kept"]);
        let archived = store.archived_topics().unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, topic.id);
        assert_eq!(archived[0].archived_at_ms, 7);

        store.restore_topic(&topic.id).unwrap();
        let state = store.load_state(machine()).unwrap();
        topic.worktree_id = left.id.clone();
        topic.worktrees = vec![left];
        topic.sessions = vec![session];
        assert_eq!(state.topic(&topic.id), Some(&topic));
        assert!(store.archived_topics().unwrap().is_empty());
    }

    #[test]
    fn upgrading_names_only_the_topics_nobody_named() {
        let conn = Connection::open_in_memory().unwrap();
        let before_naming = MIGRATIONS
            .iter()
            .position(|migration| migration.contains("auto_named"))
            .unwrap();
        for migration in &MIGRATIONS[..before_naming] {
            conn.execute_batch(migration).unwrap();
        }
        conn.pragma_update(None, "user_version", before_naming)
            .unwrap();
        conn.execute_batch(
            "INSERT INTO workspaces (id, name, path) VALUES ('w', 'repo', '/repo');
             INSERT INTO topics (id, workspace_id, name) VALUES
                 ('numbered', 'w', 'Topic #1'),
                 ('untitled', 'w', 'Topic #2'),
                 ('chosen', 'w', 'Fix login');
             INSERT INTO worktrees (id, topic_id, path, branch) VALUES
                 ('a', 'numbered', '/a', 'a'), ('b', 'untitled', '/b', 'b'), ('c', 'chosen', '/c', 'c');
             INSERT INTO sessions (id, topic_id, worktree_id, agent, mode, status, title, closed_at_ms) VALUES
                 ('closed', 'numbered', 'a', 'x', 'auto', 'done', 'A closed tab', 1),
                 ('first', 'numbered', 'a', 'x', 'auto', 'done', 'The first tab', NULL),
                 ('second', 'numbered', 'a', 'x', 'auto', 'done', 'The second tab', NULL),
                 ('none', 'untitled', 'b', 'x', 'auto', 'done', NULL, NULL),
                 ('other', 'chosen', 'c', 'x', 'auto', 'done', 'Not this', NULL);",
        )
        .unwrap();
        let store = Store::init(conn).unwrap();

        let state = store.load_state(machine()).unwrap();
        let names: Vec<_> = state.topics().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["The first tab", "Topic #2", "Fix login"]);
        // Each topic works in the folder its tabs were in.
        let folders: Vec<_> = state.topics().map(|t| t.worktree_id.0.as_str()).collect();
        assert_eq!(folders, ["a", "b", "c"]);
        assert!(store.auto_name_topic(&"untitled".into(), "Later").unwrap());
        assert!(!store.auto_name_topic(&"chosen".into(), "Later").unwrap());
    }

    #[test]
    fn mcp_servers_overrides_and_logins_survive_and_go_together() {
        let store = with_workspace();
        stored_topic(&store, "t", false);
        let server = McpServer {
            id: "x".into(),
            name: "linear".into(),
            topic_id: None,
            transport: protocol::McpTransport::Http {
                url: "https://l".into(),
                headers: BTreeMap::new(),
                oauth_client_id: None,
            },
            enabled: true,
            auth: protocol::McpAuth::LoggedIn,
        };
        store.upsert_mcp_server(&server).unwrap();
        let overrides = BTreeMap::from([(McpServerId::from("x"), false)]);
        store.set_topic_mcp(&"t".into(), &overrides).unwrap();
        let login = Login {
            token_endpoint: "https://l/token".into(),
            client_id: "c".into(),
            client_secret: None,
            resource: "https://l".into(),
            redirect_uri: "http://localhost:1/oauth/callback".into(),
            access_token: Some("a".into()),
            refresh_token: Some("r".into()),
            expires_at_ms: Some(5),
        };
        store.set_mcp_login(&server.id, &login).unwrap();

        let state = store.load_state(machine()).unwrap();
        assert_eq!(state.mcp_servers, std::slice::from_ref(&server));
        assert_eq!(state.topics().next().unwrap().mcp, overrides);
        assert_eq!(store.mcp_login(&server.id).unwrap(), Some(login));

        store.remove_mcp_server(&server.id).unwrap();
        let state = store.load_state(machine()).unwrap();
        assert!(state.mcp_servers.is_empty());
        assert!(state.topics().next().unwrap().mcp.is_empty());
        assert_eq!(store.mcp_login(&server.id).unwrap(), None);
    }

    #[test]
    fn events_after_returns_later_events_in_order() {
        let store = store();
        let workspace = Workspace {
            id: "w".into(),
            name: "repo".into(),
            path: "/repo".into(),
            topics: vec![],
        };
        store.insert_workspace(&workspace).unwrap();
        let topic = Topic {
            id: "t".into(),
            name: "topic".into(),
            worktree_id: "wt".into(),
            worktrees: vec![Worktree {
                id: "wt".into(),
                path: "/wt".into(),
                branch: "b".into(),
            }],
            sessions: vec![],
            terminals: vec![],
            mcp: BTreeMap::new(),
        };
        store.insert_topic(&workspace.id, &topic, false).unwrap();
        let session_id = SessionId::from("s");
        store
            .insert_session(
                &topic.id,
                &Session {
                    id: session_id.clone(),
                    agent: "a".into(),
                    mode: PermissionMode::Auto,
                    model: None,
                    effort: None,
                    status: SessionStatus::Ready,
                    title: None,
                    unread: false,
                    opened_at_ms: 0,
                },
            )
            .unwrap();
        let machine_id = MachineId::from("m");
        for seq in 1..=5 {
            store
                .insert_event(&SessionEvent {
                    session: SessionRef {
                        machine_id: machine_id.clone(),
                        session_id: session_id.clone(),
                    },
                    seq,
                    at_ms: 0,
                    kind: SessionEventKind::AgentMessage {
                        text: seq.to_string(),
                    },
                })
                .unwrap();
        }

        let events = store.events_after(&machine_id, &session_id, 2, 2).unwrap();
        let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [3, 4]);
        assert_eq!(store.last_seqs().unwrap(), [(session_id, 5)]);
    }
}
