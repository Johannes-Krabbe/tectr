//! Cloning a repository into a new workspace, and the GitHub repositories offered for it.
//! A clone shows in [`DaemonState::clones`](protocol::DaemonState) while it goes, so every
//! client sees it, and becomes a workspace once it's done.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use protocol::{CommandError, CommandOutput, ErrorCode, RepoClone, StateChange};

use super::{Core, Inner, invalid, resolve_path};
use crate::git::{self, CloneLogin};
use crate::github;

/// Long enough that typing through a few folders asks `gh` once, short enough that a repo
/// made on GitHub a moment ago shows up soon.
const REPOS_KEPT_FOR: Duration = Duration::from_secs(5 * 60);

/// The GitHub repositories last listed, and when.
pub(super) type ListedRepos = tokio::sync::Mutex<Option<(Instant, Vec<protocol::Repo>)>>;

impl Core {
    pub(super) async fn list_repos(&self) -> Result<CommandOutput, CommandError> {
        // Held while listing, so requests that arrive meanwhile wait for the same answer.
        let mut listed = self.repos.lock().await;
        if let Some((at, repos)) = listed.as_ref()
            && at.elapsed() < REPOS_KEPT_FOR
        {
            return Ok(CommandOutput::Repos {
                repos: repos.clone(),
                problem: None,
            });
        }
        match github::list_repos().await {
            Ok(repos) => {
                self.gh_works.store(true, Ordering::Relaxed);
                *listed = Some((Instant::now(), repos.clone()));
                Ok(CommandOutput::Repos {
                    repos,
                    problem: None,
                })
            }
            Err(problem) => {
                self.gh_works.store(false, Ordering::Relaxed);
                *listed = None;
                Ok(CommandOutput::Repos {
                    repos: vec![],
                    problem: Some(problem),
                })
            }
        }
    }

    pub(super) async fn clone_workspace(
        &self,
        url: &str,
        path: &str,
    ) -> Result<CommandOutput, CommandError> {
        let url = url.trim();
        if url.is_empty() {
            return Err(invalid("cloning needs the repository's address"));
        }
        let resolved = resolve_path(path)?;
        if tokio::fs::symlink_metadata(&resolved).await.is_ok() {
            return Err(invalid(format!("{} already exists", resolved.display())));
        }
        let path_text = resolved.display().to_string();
        let mut going = {
            let mut inner = self.lock();
            if inner
                .state
                .clones
                .iter()
                .any(|c| c.path == path_text && c.error.is_none())
            {
                return Err(CommandError {
                    code: ErrorCode::Busy,
                    message: format!("{path_text} is already being cloned"),
                });
            }
            // Trying again replaces what failed there.
            inner.state.clones.retain(|c| c.path != path_text);
            inner.state.clones.push(RepoClone {
                name: resolved.file_name().map_or_else(
                    || path_text.clone(),
                    |name| name.to_string_lossy().into_owned(),
                ),
                path: path_text.clone(),
                url: url.to_owned(),
                phase: None,
                percent: None,
                error: None,
            });
            inner.emit_clones();
            Going {
                core: self,
                path: path_text.clone(),
                failed: None,
            }
        };

        // `gh` having listed repos means it's logged in, and can log git in too.
        let login = if github::is_github_https(url) && self.gh_works.load(Ordering::Relaxed) {
            CloneLogin::Gh
        } else {
            CloneLogin::GitConfig
        };
        let cloned = git::clone(url, &resolved, login, |phase, percent| {
            let mut inner = self.lock();
            let Some(clone) = inner.state.clones.iter_mut().find(|c| c.path == path_text) else {
                return;
            };
            if clone.phase.as_deref() == Some(phase) && clone.percent == Some(percent) {
                return;
            }
            clone.phase = Some(phase.to_owned());
            clone.percent = Some(percent);
            inner.emit_clones();
        })
        .await;
        if let Err(error) = cloned {
            let message = format!("Couldn't clone {url}: {error:#}");
            going.failed = Some(message.clone());
            return Err(invalid(message));
        }
        // Added before the clone goes, so the two never both disappear for a moment.
        let added = self.add_workspace(&path_text).await;
        drop(going);
        added
    }

    pub(super) fn dismiss_clone(&self, path: &str) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let before = inner.state.clones.len();
        inner
            .state
            .clones
            .retain(|c| c.path != path || c.error.is_none());
        if inner.state.clones.len() == before {
            return Err(invalid(format!("no failed clone at {path}")));
        }
        inner.emit_clones();
        Ok(CommandOutput::Done)
    }
}

impl Inner {
    fn emit_clones(&mut self) {
        let clones = self.state.clones.clone();
        self.emit(StateChange::ClonesChanged { clones });
    }
}

/// Takes a clone out of the state however its command ends, including the daemon stopping,
/// unless it failed, which stays to say why until someone dismisses it.
struct Going<'a> {
    core: &'a Core,
    path: String,
    failed: Option<String>,
}

impl Drop for Going<'_> {
    fn drop(&mut self) {
        // Panicking again while a panic unwinds would abort the daemon.
        let Ok(mut inner) = self.core.inner.lock() else {
            return;
        };
        match self.failed.take() {
            Some(error) => {
                if let Some(clone) = inner.state.clones.iter_mut().find(|c| c.path == self.path) {
                    clone.error = Some(error);
                    clone.phase = None;
                    clone.percent = None;
                }
            }
            None => inner.state.clones.retain(|c| c.path != self.path),
        }
        inner.emit_clones();
    }
}
