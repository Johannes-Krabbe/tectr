//! Diff tabs. What one shows is read from git when a client asks, and kept while the folder is
//! watched: a change on disk reads it again, and only a real change gives the tabs a new
//! revision, so a build writing ignored files sends nobody anything.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use protocol::{
    CommandError, CommandOutput, Diff, DiffContent, DiffId, DiffScope, Hunk, StateChange, TopicId,
};
use tokio::sync::Notify;

use super::{Core, Inner, internal, invalid, not_found, now_ms};
use crate::git::actions::{self, HunkAction};
use crate::git::diff::{self, Changes};

/// What a diff tab can do to its folder.
pub(super) enum DiffAction {
    StageFiles {
        paths: Vec<String>,
        stage: bool,
    },
    Hunk {
        path: String,
        hunk: Hunk,
        action: HunkAction,
    },
    DiscardFiles {
        paths: Vec<String>,
    },
    Commit {
        message: String,
        amend: bool,
    },
}

/// Changes come in bursts, like a save or a checkout, and one read covers the whole burst.
const SETTLE: Duration = Duration::from_millis(250);

/// The watching and the last read of every folder a diff tab shows.
#[derive(Default)]
pub(super) struct DiffWatch {
    /// `None` when the folder couldn't be watched, so it isn't tried again on every change.
    watchers: HashMap<String, Option<RecommendedWatcher>>,
    cache: HashMap<(String, DiffScope), Cached>,
    dirty: Arc<Dirty>,
    /// Counts up from when the daemon started, so a revision from before a restart is never
    /// taken for one after it.
    last_revision: u64,
    /// Orders reads, so one that started earlier and finished later can't replace a newer one.
    reads: u64,
}

impl DiffWatch {
    pub(super) fn new() -> Self {
        Self {
            last_revision: now_ms().max(0) as u64,
            ..Self::default()
        }
    }
}

struct Cached {
    read: u64,
    revision: u64,
    result: Result<Arc<Changes>, String>,
}

impl Cached {
    fn answer(&self) -> Result<CommandOutput, CommandError> {
        match &self.result {
            Ok(changes) => Ok(CommandOutput::DiffContent {
                content: DiffContent {
                    revision: self.revision,
                    base: changes.base.clone(),
                    head_message: changes.head_message.clone(),
                    files: changes.files.clone(),
                },
            }),
            Err(message) => Err(invalid(message.clone())),
        }
    }
}

/// Folders that changed on disk since they were last read.
#[derive(Default)]
struct Dirty {
    folders: Mutex<HashSet<String>>,
    wake: Notify,
}

impl Dirty {
    fn mark(&self, folder: &str) {
        self.folders
            .lock()
            .expect("nothing panics holding this")
            .insert(folder.to_owned());
        self.wake.notify_one();
    }

    fn take(&self) -> HashSet<String> {
        std::mem::take(&mut self.folders.lock().expect("nothing panics holding this"))
    }
}

impl Core {
    pub(super) fn open_diff(
        &self,
        topic_id: &TopicId,
        scope: DiffScope,
    ) -> Result<CommandOutput, CommandError> {
        known(&scope)?;
        let mut inner = self.lock();
        let topic = inner
            .state
            .topic(topic_id)
            .ok_or_else(|| not_found("topic", topic_id))?;
        let folder = topic.worktree().map(|w| w.path.clone()).unwrap_or_default();
        let diff = Diff {
            id: DiffId(crate::new_id()),
            revision: inner.cached_revision(&folder, &scope),
            scope,
            opened_at_ms: now_ms(),
        };
        inner.store.insert_diff(topic_id, &diff).map_err(internal)?;
        inner.emit(StateChange::DiffAdded {
            topic_id: topic_id.clone(),
            diff: diff.clone(),
        });
        Ok(CommandOutput::Diff {
            topic_id: topic_id.clone(),
            diff,
        })
    }

    pub(super) fn close_diff(&self, diff_id: &DiffId) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        if inner.state.diff(diff_id).is_none() {
            return Err(not_found("diff", diff_id));
        }
        inner
            .store
            .close_diff(diff_id, now_ms())
            .map_err(internal)?;
        inner.emit(StateChange::DiffRemoved {
            diff_id: diff_id.clone(),
        });
        Ok(CommandOutput::Done)
    }

    pub(super) fn set_diff_scope(
        &self,
        diff_id: &DiffId,
        scope: DiffScope,
    ) -> Result<CommandOutput, CommandError> {
        known(&scope)?;
        let mut inner = self.lock();
        let (topic, diff) = inner
            .state
            .diff(diff_id)
            .ok_or_else(|| not_found("diff", diff_id))?;
        let folder = topic.worktree().map(|w| w.path.clone()).unwrap_or_default();
        let mut diff = diff.clone();
        if diff.scope == scope {
            return Ok(CommandOutput::Done);
        }
        inner
            .store
            .set_diff_scope(diff_id, &scope)
            .map_err(internal)?;
        diff.revision = inner.cached_revision(&folder, &scope);
        diff.scope = scope;
        inner.emit(StateChange::DiffChanged { diff });
        Ok(CommandOutput::Done)
    }

    pub(super) async fn read_diff(&self, diff_id: &DiffId) -> Result<CommandOutput, CommandError> {
        let (folder, scope) = {
            let inner = self.lock();
            let (topic, diff) = inner
                .state
                .diff(diff_id)
                .ok_or_else(|| not_found("diff", diff_id))?;
            let folder = topic
                .worktree()
                .ok_or_else(|| internal(anyhow::anyhow!("the topic has no folder")))?
                .path
                .clone();
            if let Some(cached) = inner.diffs.cache.get(&(folder.clone(), diff.scope.clone())) {
                return cached.answer();
            }
            (folder, diff.scope.clone())
        };
        self.read_folder(&folder, scope).await
    }

    /// Runs one of a diff tab's git actions in its folder, then reads the folder again before
    /// answering, rather than after the watcher's pause, so the tab shows the result as soon
    /// as the command returns.
    pub(super) async fn diff_action(
        &self,
        diff_id: &DiffId,
        action: DiffAction,
    ) -> Result<CommandOutput, CommandError> {
        let folder = self.diff_folder(diff_id)?;
        let dir = Path::new(&folder);
        let result = match &action {
            DiffAction::StageFiles { paths, stage: true } => actions::stage_files(dir, paths).await,
            DiffAction::StageFiles {
                paths,
                stage: false,
            } => actions::unstage_files(dir, paths).await,
            DiffAction::Hunk { path, hunk, action } => {
                actions::apply_hunk(dir, path, hunk, *action).await
            }
            DiffAction::DiscardFiles { paths } => actions::discard_files(dir, paths).await,
            DiffAction::Commit { message, amend } => actions::commit(dir, message, *amend).await,
        };
        self.refresh_folder(&folder).await;
        result.map_err(|error| invalid(format!("{error:#}")))?;
        Ok(CommandOutput::Done)
    }

    fn diff_folder(&self, diff_id: &DiffId) -> Result<String, CommandError> {
        let inner = self.lock();
        let (topic, _) = inner
            .state
            .diff(diff_id)
            .ok_or_else(|| not_found("diff", diff_id))?;
        Ok(topic
            .worktree()
            .ok_or_else(|| internal(anyhow::anyhow!("the topic has no folder")))?
            .path
            .clone())
    }

    /// Reads folders again once they've settled after changing on disk, for as long as the
    /// daemon runs.
    pub async fn refresh_diffs(self: Arc<Self>) {
        let dirty = self.lock().diffs.dirty.clone();
        loop {
            dirty.wake.notified().await;
            tokio::time::sleep(SETTLE).await;
            for folder in dirty.take() {
                self.refresh_folder(&folder).await;
            }
        }
    }

    /// Reads a folder again for every scope a tab shows it in.
    async fn refresh_folder(&self, folder: &str) {
        let scopes: HashSet<DiffScope> = {
            let mut inner = self.lock();
            let scopes: HashSet<DiffScope> = inner
                .state
                .topics()
                .filter(|t| t.worktree().is_some_and(|w| w.path == folder))
                .flat_map(|t| t.diffs.iter().map(|d| d.scope.clone()))
                .collect();
            // A scope nobody shows would go stale unseen, so it's read again when asked.
            inner
                .diffs
                .cache
                .retain(|(f, scope), _| f != folder || scopes.contains(scope));
            scopes
        };
        for scope in scopes {
            // A failure is kept as the answer, which the tab shows.
            let _ = self.read_folder(folder, scope).await;
        }
    }

    async fn read_folder(
        &self,
        folder: &str,
        scope: DiffScope,
    ) -> Result<CommandOutput, CommandError> {
        let read = {
            let mut inner = self.lock();
            inner.diffs.reads += 1;
            inner.diffs.reads
        };
        let result = diff::read(Path::new(folder), &scope)
            .await
            .map(Arc::new)
            .map_err(|error| format!("{error:#}"));
        self.lock().keep_read(folder, scope, read, result)
    }
}

impl Inner {
    fn cached_revision(&self, folder: &str, scope: &DiffScope) -> u64 {
        self.diffs
            .cache
            .get(&(folder.to_owned(), scope.clone()))
            .map_or(0, |cached| cached.revision)
    }

    /// Keeps a read while its folder is watched, and gives the tabs showing it a new revision
    /// if it differs from the last.
    fn keep_read(
        &mut self,
        folder: &str,
        scope: DiffScope,
        read: u64,
        result: Result<Arc<Changes>, String>,
    ) -> Result<CommandOutput, CommandError> {
        let key = (folder.to_owned(), scope);
        if let Some(cached) = self.diffs.cache.get_mut(&key) {
            if cached.read > read {
                return cached.answer();
            }
            if cached.result == result {
                cached.read = read;
                return cached.answer();
            }
        }
        self.diffs.last_revision += 1;
        let cached = Cached {
            read,
            revision: self.diffs.last_revision,
            result,
        };
        let answer = cached.answer();
        let changed: Vec<Diff> = self
            .state
            .topics()
            .filter(|t| t.worktree().is_some_and(|w| w.path == folder))
            .flat_map(|t| &t.diffs)
            .filter(|d| d.scope == key.1 && d.revision != cached.revision)
            .map(|d| Diff {
                revision: cached.revision,
                ..d.clone()
            })
            .collect();
        // Unwatched, nothing would keep it current.
        if self.diffs.watchers.contains_key(folder) {
            self.diffs.cache.insert(key, cached);
        }
        for diff in changed {
            self.emit(StateChange::DiffChanged { diff });
        }
        answer
    }

    /// Watches exactly the folders some diff tab shows. Runs after every change, because tabs
    /// follow their topic from folder to folder.
    pub(super) fn sync_diff_watchers(&mut self) {
        let wanted: HashSet<String> = self
            .state
            .topics()
            .filter(|t| !t.diffs.is_empty())
            .filter_map(|t| t.worktree().map(|w| w.path.clone()))
            .collect();
        let watched = &self.diffs.watchers;
        if watched.len() == wanted.len() && wanted.iter().all(|f| watched.contains_key(f)) {
            return;
        }
        self.diffs
            .watchers
            .retain(|folder, _| wanted.contains(folder));
        self.diffs
            .cache
            .retain(|(folder, _), _| wanted.contains(folder));
        for folder in wanted {
            if self.diffs.watchers.contains_key(&folder) {
                continue;
            }
            let watcher = watch(&folder, self.diffs.dirty.clone())
                .inspect_err(|error| tracing::warn!(folder, "can't watch for changes: {error:#}"))
                .ok();
            self.diffs.watchers.insert(folder.clone(), watcher);
            // A tab that just arrived here, with its topic, shows another folder's revision.
            self.diffs.dirty.mark(&folder);
        }
    }
}

fn known(scope: &DiffScope) -> Result<(), CommandError> {
    match scope {
        DiffScope::Unknown => Err(invalid("this daemon doesn't know that kind of diff")),
        _ => Ok(()),
    }
}

fn watch(folder: &str, dirty: Arc<Dirty>) -> notify::Result<RecommendedWatcher> {
    let key = folder.to_owned();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // An error can mean events were lost, so it counts as a change.
        if event.is_ok_and(|event| matches!(event.kind, EventKind::Access(_))) {
            return;
        }
        dirty.mark(&key);
    })?;
    watcher.watch(Path::new(folder), RecursiveMode::Recursive)?;
    // A worktree's index and HEAD live in the main repo's .git, outside the folder.
    if let Some(git_dir) = linked_git_dir(Path::new(folder)) {
        watcher.watch(&git_dir, RecursiveMode::NonRecursive)?;
    }
    Ok(watcher)
}

/// Where a worktree's own git files are, from the `.git` file in it.
fn linked_git_dir(folder: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(folder.join(".git")).ok()?;
    let path = PathBuf::from(text.trim().strip_prefix("gitdir: ")?);
    Some(if path.is_absolute() {
        path
    } else {
        folder.join(path)
    })
}
