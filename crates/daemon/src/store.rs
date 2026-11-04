//! SQLite persistence. Workspaces, topics, and session events survive a daemon
//! restart, and resuming a client's event stream reads from here.

use std::path::Path;

use anyhow::{Context, Result};
use protocol::{
    Approval, ApprovalId, DaemonState, Machine, MachineId, PermissionMode, Session, SessionEvent,
    SessionEventKind, SessionId, SessionRef, SessionStatus, Topic, TopicId, Workspace, WorkspaceId,
    Worktree, WorktreeId,
};
use rusqlite::{Connection, OptionalExtension, params};

const MIGRATIONS: &[&str] = &["
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
"];

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
                "SELECT id, name FROM topics WHERE workspace_id = ?1 ORDER BY rowid",
                [&workspace.id.0],
                |row| {
                    Ok(Topic {
                        id: TopicId(row.get(0)?),
                        name: row.get(1)?,
                        worktrees: vec![],
                        sessions: vec![],
                    })
                },
            )?;
            for topic in &mut workspace.topics {
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
                    "SELECT id, worktree_id, agent, mode, model, status, title
                     FROM sessions WHERE topic_id = ?1 ORDER BY rowid",
                    [&topic.id.0],
                    |row| {
                        let mode: String = row.get(3)?;
                        let status: String = row.get(5)?;
                        Ok(Session {
                            id: SessionId(row.get(0)?),
                            worktree_id: WorktreeId(row.get(1)?),
                            agent: row.get(2)?,
                            mode: from_column::<PermissionMode>(&mode),
                            model: row.get(4)?,
                            status: from_column::<SessionStatus>(&status),
                            title: row.get(6)?,
                        })
                    },
                )?;
            }
        }
        Ok(DaemonState {
            machine,
            workspaces,
            approvals: vec![],
        })
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

    pub fn insert_topic(&self, workspace_id: &WorkspaceId, topic: &Topic) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO topics (id, workspace_id, name) VALUES (?1, ?2, ?3)",
            params![topic.id.0, workspace_id.0, topic.name],
        )?;
        for worktree in &topic.worktrees {
            tx.execute(
                "INSERT INTO worktrees (id, topic_id, path, branch) VALUES (?1, ?2, ?3, ?4)",
                params![worktree.id.0, topic.id.0, worktree.path, worktree.branch],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn insert_session(&self, topic_id: &TopicId, session: &Session) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions (id, topic_id, worktree_id, agent, mode, model, status, title)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session.id.0,
                topic_id.0,
                session.worktree_id.0,
                session.agent,
                to_column(&session.mode),
                session.model,
                to_column(&session.status),
                session.title,
            ],
        )?;
        Ok(())
    }

    pub fn update_session(&self, session: &Session) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET status = ?2, title = ?3 WHERE id = ?1",
            params![session.id.0, to_column(&session.status), session.title],
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
    fn machine_id_is_stable() {
        let store = store();
        assert_eq!(store.machine_id().unwrap(), store.machine_id().unwrap());
    }

    #[test]
    fn state_round_trips_through_the_database() {
        let store = store();
        let session = Session {
            id: "s".into(),
            worktree_id: "wt".into(),
            agent: "claude".into(),
            mode: PermissionMode::Auto,
            model: Some("haiku".into()),
            status: SessionStatus::WaitingForYou,
            title: None,
        };
        let topic = Topic {
            id: "t".into(),
            name: "topic".into(),
            worktrees: vec![Worktree {
                id: "wt".into(),
                path: "/wt".into(),
                branch: "tectr/topic".into(),
            }],
            sessions: vec![],
        };
        let workspace = Workspace {
            id: "w".into(),
            name: "repo".into(),
            path: "/repo".into(),
            topics: vec![],
        };
        store.insert_workspace(&workspace).unwrap();
        store.insert_topic(&workspace.id, &topic).unwrap();
        store.insert_session(&topic.id, &session).unwrap();

        let state = store.load_state(machine()).unwrap();
        assert_eq!(state.workspaces[0].topics[0].worktrees, topic.worktrees);
        assert_eq!(state.workspaces[0].topics[0].sessions, vec![session]);
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
            worktrees: vec![Worktree {
                id: "wt".into(),
                path: "/wt".into(),
                branch: "b".into(),
            }],
            sessions: vec![],
        };
        store.insert_topic(&workspace.id, &topic).unwrap();
        let session_id = SessionId::from("s");
        store
            .insert_session(
                &topic.id,
                &Session {
                    id: session_id.clone(),
                    worktree_id: "wt".into(),
                    agent: "a".into(),
                    mode: PermissionMode::Auto,
                    model: None,
                    status: SessionStatus::Ready,
                    title: None,
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
