//! Closing a workspace, opening it again, and forgetting it. A closed workspace keeps
//! everything in the database and on disk, and only its processes end.

use std::collections::BTreeMap;
use std::path::PathBuf;

use protocol::{
    ArchiveRisk, CommandError, CommandOutput, DaemonState, ErrorCode, McpServerId, SessionStatus,
    StateChange, WorkspaceId, WorkspaceMcp, WorktreeRisk,
};

use super::{Core, discard_worktrees, internal, invalid, not_found, now_ms, owned_worktrees};
use crate::git;

impl Core {
    pub(super) fn close_workspace(
        &self,
        id: &WorkspaceId,
        force: bool,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let workspace = inner
            .state
            .workspace(id)
            .cloned()
            .ok_or_else(|| not_found("workspace", id))?;
        let sessions = || workspace.topics.iter().flat_map(|t| &t.sessions);
        let running = sessions().filter(|s| inner.is_running(s)).count();
        if running > 0 && !force {
            let agents = if running == 1 {
                "an agent".to_owned()
            } else {
                format!("{running} agents")
            };
            return Err(CommandError {
                code: ErrorCode::Busy,
                message: format!(
                    "{agents} in {} still running, so closing it needs confirming",
                    workspace.name
                ),
            });
        }
        inner
            .store
            .close_workspace(id, now_ms())
            .map_err(internal)?;
        for session in sessions() {
            let running = inner.is_running(session);
            if let Some(task) = inner.stop_agent(&session.id) {
                inner.keep_closing(task);
            }
            if running {
                inner.set_status(&session.id, SessionStatus::Stopped);
            }
        }
        // What the shells showed comes back with the workspace, like an archived topic's.
        for terminal in workspace.topics.iter().flat_map(|t| &t.terminals) {
            inner.stop_terminal(&terminal.id, true);
        }
        inner.emit(StateChange::WorkspaceRemoved {
            workspace_id: id.clone(),
        });
        Ok(CommandOutput::Done)
    }

    pub(super) fn open_workspace(&self, id: &WorkspaceId) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        if let Some(open) = inner.state.workspace(id) {
            return Ok(CommandOutput::Workspace {
                workspace: open.clone(),
            });
        }
        let mut workspace = inner
            .store
            .closed_workspace(id)
            .map_err(internal)?
            .ok_or_else(|| not_found("workspace", id))?;
        // The library can lose servers and profiles while the workspace is away.
        let mcp = still_in_library(&inner.state, &workspace.mcp);
        if mcp != workspace.mcp {
            inner.store.set_workspace_mcp(id, &mcp).map_err(internal)?;
            workspace.mcp = mcp;
        }
        let at = inner.store.workspace_position(id).map_err(internal)?;
        inner.store.open_workspace(id).map_err(internal)?;
        // Closing stopped its agents, and one stopped on its way out may not have said so.
        for session in workspace.topics.iter_mut().flat_map(|t| &mut t.sessions) {
            if matches!(
                session.status,
                SessionStatus::Starting | SessionStatus::Working | SessionStatus::WaitingForYou
            ) {
                session.status = SessionStatus::Stopped;
                inner.store.update_session(session).map_err(internal)?;
            }
        }
        let mut commands = vec![];
        for topic in &workspace.topics {
            commands.extend(inner.store.topic_commands(&topic.id).map_err(internal)?);
        }
        inner.emit(StateChange::WorkspaceAdded {
            workspace: workspace.clone(),
            at: Some(at),
        });
        for (session_id, commands) in commands {
            inner.emit(StateChange::CommandsChanged {
                session_id,
                commands,
            });
        }
        Ok(CommandOutput::Workspace { workspace })
    }

    pub(super) fn closed_workspaces(&self) -> Result<CommandOutput, CommandError> {
        let workspaces = self.lock().store.closed_workspaces().map_err(internal)?;
        Ok(CommandOutput::ClosedWorkspaces { workspaces })
    }

    /// Only a closed workspace can go, so no agent or shell of its is running. Its worktrees
    /// go behind the answer, like an archived topic's.
    pub(super) async fn remove_workspace(
        &self,
        id: &WorkspaceId,
        force: bool,
    ) -> Result<CommandOutput, CommandError> {
        let (repo, owned) = {
            let inner = self.lock();
            if inner.state.workspace(id).is_some() {
                return Err(invalid("close the workspace before removing it"));
            }
            let workspace = inner
                .store
                .closed_workspace(id)
                .map_err(internal)?
                .ok_or_else(|| not_found("workspace", id))?;
            let repo = PathBuf::from(&workspace.path);
            let mut owned = vec![];
            for topic in inner.store.every_topic(id).map_err(internal)? {
                let made = inner.store.made_branches(&topic.id).map_err(internal)?;
                owned.extend(owned_worktrees(&repo, &topic, &made));
            }
            // An archived topic's folder is already gone.
            owned.retain(|(path, _)| path.exists());
            (repo, owned)
        };
        if !force {
            let mut worktrees = vec![];
            for (path, branch) in &owned {
                let uncommitted = git::uncommitted(path).await.map_err(internal)?;
                let unpushed = git::unpushed(&repo, branch).await.map_err(internal)?;
                if uncommitted > 0 || unpushed > 0 {
                    worktrees.push(WorktreeRisk {
                        path: path.display().to_string(),
                        branch: branch.clone(),
                        uncommitted,
                        unpushed,
                    });
                }
            }
            if !worktrees.is_empty() {
                let risk = ArchiveRisk {
                    running: 0,
                    worktrees,
                };
                return Ok(CommandOutput::ArchiveRisk { risk });
            }
        }

        let changing = self.worktree_changes.clone().lock_owned().await;
        let mut inner = self.lock();
        // It may have been opened while git was asked.
        if inner.state.workspace(id).is_some() {
            return Err(invalid("close the workspace before removing it"));
        }
        let topics = inner.store.every_topic(id).map_err(internal)?;
        inner.store.remove_workspace(id).map_err(internal)?;
        for topic in &topics {
            for session in &topic.sessions {
                if let Err(error) = inner.clear_draft(&session.id) {
                    tracing::error!(id = %session.id, "failed to clear a draft: {error:#}");
                }
            }
            for terminal in &topic.terminals {
                let _ = std::fs::remove_file(self.terminal_history(&terminal.id));
            }
        }
        let folder = self.data_dir.join("worktrees").join(&id.0);
        let cleanup = tokio::spawn(async move {
            discard_worktrees(repo, owned, vec![]).await;
            // Only once it's empty, so nothing left in it is lost.
            let _ = tokio::fs::remove_dir(folder).await;
            drop(changing);
        });
        inner.keep_closing(cleanup);
        Ok(CommandOutput::Done)
    }
}

/// A workspace's servers and profiles that the library still has.
fn still_in_library(
    state: &DaemonState,
    mcp: &BTreeMap<McpServerId, WorkspaceMcp>,
) -> BTreeMap<McpServerId, WorkspaceMcp> {
    mcp.iter()
        .filter_map(|(id, entry)| {
            let server = state.mcp_server(id)?;
            let profile_id = entry
                .profile_id
                .clone()
                .filter(|profile| server.profile(profile).is_some());
            Some((
                id.clone(),
                WorkspaceMcp {
                    profile_id,
                    ..entry.clone()
                },
            ))
        })
        .collect()
}
