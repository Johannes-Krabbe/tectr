//! Background work an agent leaves running, such as a command it started in the background.
//! Claude Code reports its tasks in its own messages, which its adapter passes on when asked
//! to (see `emitRawSDKMessages` in `acp.rs`). This turns them into [`BackgroundTask`]s.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};
use protocol::{BackgroundTask, MAX_TASK_OUTPUT_BYTES, TaskStatus};
use serde_json::Value;

/// The kinds of Claude Code messages tectr asks its adapter to pass on.
pub const CLAUDE_MESSAGES: &[&str] = &[
    "task_started",
    "task_updated",
    "task_notification",
    "background_tasks_changed",
    "session_state_changed",
];

/// What one of Claude Code's messages says about the session.
#[derive(Debug, PartialEq)]
pub enum Report {
    Task(BackgroundTask),
    /// The agent started or stopped working, whether or not tectr asked it to.
    Working(bool),
}

/// The tasks of one agent process, as its messages have described them so far.
#[derive(Default)]
pub struct Tasks {
    tasks: HashMap<String, BackgroundTask>,
    /// What each tool call ran, for the tasks that come from one.
    commands: HashMap<String, String>,
    /// Where Claude Code writes each task's output, by task ID, from the text of tool results.
    outputs: HashMap<String, String>,
}

impl Tasks {
    /// Takes note of a tool call's input and output, which name a task's command and file.
    pub fn note_tool_call(&mut self, id: &str, input: Option<&Value>, texts: &[&str]) {
        if let Some(command) = input.and_then(|i| i.get("command")).and_then(Value::as_str) {
            self.commands.insert(id.to_owned(), command.to_owned());
        }
        for text in texts {
            for path in output_paths(text) {
                if let Some(task) = task_of_path(&path) {
                    self.outputs.insert(task.to_owned(), path.clone());
                }
            }
        }
    }

    /// The tasks a message changed, and whether the agent's activity changed. Tasks that are
    /// subagents are left out, because subagents report themselves, and so is housekeeping
    /// Claude Code marks as not being activity.
    pub fn read(&mut self, message: &Value) -> Vec<Report> {
        if message.get("type").and_then(Value::as_str) != Some("system") {
            return vec![];
        }
        let field = |key: &str| message.get(key).and_then(Value::as_str);
        match field("subtype") {
            Some("session_state_changed") => match field("state") {
                Some("running") => vec![Report::Working(true)],
                Some("idle") => vec![Report::Working(false)],
                _ => vec![],
            },
            Some("task_started") => {
                let Some(id) = field("task_id") else {
                    return vec![];
                };
                if is_hidden(message) {
                    return vec![];
                }
                let tool_call = field("tool_use_id").map(str::to_owned);
                let kind = kind_name(field("task_type").unwrap_or_default());
                let task = BackgroundTask {
                    id: id.to_owned(),
                    command: tool_call
                        .as_ref()
                        .and_then(|call| self.commands.get(call))
                        .cloned(),
                    can_stop: kind == "shell",
                    kind,
                    description: field("description").unwrap_or_default().to_owned(),
                    status: TaskStatus::Running,
                    summary: None,
                    output_file: self.outputs.get(id).cloned(),
                    tool_call,
                };
                self.store(task)
            }
            Some("task_updated") => {
                let (Some(id), Some(patch)) = (field("task_id"), message.get("patch")) else {
                    return vec![];
                };
                let Some(mut task) = self.tasks.get(id).cloned() else {
                    return vec![];
                };
                if let Some(status) = patch.get("status").and_then(Value::as_str) {
                    task.status = status_of(status);
                }
                if let Some(description) = patch.get("description").and_then(Value::as_str) {
                    description.clone_into(&mut task.description);
                }
                if let Some(error) = patch.get("error").and_then(Value::as_str) {
                    task.summary = Some(error.to_owned());
                }
                self.store(task)
            }
            Some("task_notification") => {
                let Some(id) = field("task_id") else {
                    return vec![];
                };
                let Some(mut task) = self.tasks.get(id).cloned() else {
                    return vec![];
                };
                task.status = status_of(field("status").unwrap_or_default());
                task.summary = field("summary")
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_owned)
                    .or(task.summary);
                if let Some(path) = field("output_file").filter(|p| !p.is_empty()) {
                    task.output_file = Some(path.to_owned());
                }
                self.store(task)
            }
            // The whole set of live tasks. One that's gone without saying how it ended ended
            // anyway, so it stops showing as running.
            Some("background_tasks_changed") => {
                let live: Vec<&Value> = message
                    .get("tasks")
                    .and_then(Value::as_array)
                    .map(|tasks| tasks.iter().collect())
                    .unwrap_or_default();
                let mut reports = vec![];
                for entry in &live {
                    let Some(id) = entry.get("task_id").and_then(Value::as_str) else {
                        continue;
                    };
                    if self.tasks.contains_key(id) || is_hidden(entry) {
                        continue;
                    }
                    let kind = kind_name(
                        entry
                            .get("task_type")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    );
                    reports.extend(
                        self.store(BackgroundTask {
                            id: id.to_owned(),
                            can_stop: kind == "shell",
                            kind,
                            description: entry
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            command: None,
                            status: TaskStatus::Running,
                            summary: None,
                            output_file: self.outputs.get(id).cloned(),
                            tool_call: None,
                        }),
                    );
                }
                let ended: Vec<BackgroundTask> = self
                    .tasks
                    .values()
                    .filter(|task| task.status == TaskStatus::Running)
                    .filter(|task| {
                        !live
                            .iter()
                            .any(|e| e.get("task_id").and_then(Value::as_str) == Some(&task.id))
                    })
                    .cloned()
                    .collect();
                for mut task in ended {
                    task.status = TaskStatus::Completed;
                    reports.extend(self.store(task));
                }
                reports
            }
            _ => vec![],
        }
    }

    /// Fills in a task's file once a tool result names it, which can come after the task.
    pub fn output_found(&mut self) -> Vec<Report> {
        let found: Vec<BackgroundTask> = self
            .tasks
            .values()
            .filter(|task| task.output_file.is_none())
            .filter_map(|task| {
                let path = self.outputs.get(&task.id)?;
                Some(BackgroundTask {
                    output_file: Some(path.clone()),
                    ..task.clone()
                })
            })
            .collect();
        found
            .into_iter()
            .flat_map(|task| self.store(task))
            .collect()
    }

    fn store(&mut self, task: BackgroundTask) -> Vec<Report> {
        if self.tasks.get(&task.id) == Some(&task) {
            return vec![];
        }
        self.tasks.insert(task.id.clone(), task.clone());
        vec![Report::Task(task)]
    }
}

/// Subagents have their own place, and Claude Code marks housekeeping as not activity.
fn is_hidden(message: &Value) -> bool {
    let flag = |key: &str| message.get(key).and_then(Value::as_bool) == Some(true);
    message.get("task_type").and_then(Value::as_str) == Some("local_agent")
        || message
            .get("subagent_type")
            .and_then(Value::as_str)
            .is_some()
        || flag("ambient")
        || flag("skip_transcript")
}

/// Claude Code's task types, in the words it uses for them itself.
fn kind_name(task_type: &str) -> String {
    match task_type {
        "local_bash" => "shell",
        "local_workflow" => "workflow",
        "monitor_mcp" | "monitor" => "monitor",
        "mcp_task" => "mcp",
        "remote_agent" => "remote agent",
        "" => "task",
        other => other,
    }
    .to_owned()
}

fn status_of(status: &str) -> TaskStatus {
    match status {
        "pending" | "running" | "paused" => TaskStatus::Running,
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        "killed" | "stopped" => TaskStatus::Stopped,
        _ => TaskStatus::Unknown,
    }
}

/// Claude Code says where a task's output goes as "...: /path/tasks/<id>.output".
fn output_paths(text: &str) -> Vec<String> {
    let mut paths = vec![];
    for line in text.lines() {
        let mut rest = line;
        while let Some(end) = rest.find(".output") {
            let (before, after) = rest.split_at(end + ".output".len());
            let start = before.rfind(": ").map_or(0, |at| at + 2);
            let path = before[start..].trim();
            if path.starts_with('/') && path.contains("/tasks/") {
                paths.push(path.to_owned());
            }
            rest = after;
        }
    }
    paths
}

fn task_of_path(path: &str) -> Option<&str> {
    let (_, name) = path.rsplit_once("/tasks/")?;
    name.strip_suffix(".output")
}

/// The end of a task's output file, and whether there was more before it.
pub fn read_output(path: &Path) -> Result<(String, bool)> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("can't open {}", path.display()))?;
    let length = file.metadata()?.len();
    let limit = MAX_TASK_OUTPUT_BYTES as u64;
    let truncated = length > limit;
    if truncated {
        file.seek(SeekFrom::Start(length - limit))?;
    }
    let mut bytes = vec![];
    file.read_to_end(&mut bytes)?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    // A cut can land mid-line, which would read as garbage.
    if truncated && let Some(newline) = text.find('\n') {
        text.drain(..=newline);
    }
    Ok((text, truncated))
}

/// Stops the processes writing a task's output, which are the command and whatever it
/// started. Claude Code's adapter can't stop a task for a client like tectr, so this does
/// what the command's own process group would get from a terminal's Ctrl+C. Processes that
/// are this daemon's agents, or the daemon itself, are never touched, even if they have the
/// file open. Returns how many it stopped, which is none once the task has ended.
pub async fn stop(path: &Path) -> Result<usize> {
    let output = tokio::process::Command::new("lsof")
        .arg("-t")
        .arg("--")
        .arg(path)
        .output()
        .await
        .context("couldn't run lsof")?;
    let pids: Vec<i32> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect();
    let daemon = std::process::id() as i32;
    let mut stopped = 0;
    for pid in pids {
        if pid == daemon || is_agent(pid, daemon).await {
            continue;
        }
        let group = group_of(pid).await;
        // A command Claude Code started leads its own process group, so the group takes its
        // children with it. Anything else only gets stopped itself.
        let target = match group {
            Some(group) if group == pid => format!("-{pid}"),
            _ => pid.to_string(),
        };
        let killed = tokio::process::Command::new("kill")
            .args(["-TERM", "--", &target])
            .status()
            .await?;
        if killed.success() {
            stopped += 1;
        }
    }
    Ok(stopped)
}

/// The agent a daemon starts is its child, and the agent's own program is that child's child.
/// A task's command is further down.
async fn is_agent(pid: i32, daemon: i32) -> bool {
    let Some(parent) = parent_of(pid).await else {
        return true;
    };
    if parent == daemon {
        return true;
    }
    parent_of(parent).await == Some(daemon)
}

async fn parent_of(pid: i32) -> Option<i32> {
    ps_field(pid, "ppid=").await
}

async fn group_of(pid: i32) -> Option<i32> {
    ps_field(pid, "pgid=").await
}

async fn ps_field(pid: i32, field: &str) -> Option<i32> {
    let output = tokio::process::Command::new("ps")
        .args(["-o", field, "-p", &pid.to_string()])
        .output()
        .await
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(reports: &[Report]) -> &BackgroundTask {
        match reports.last() {
            Some(Report::Task(task)) => task,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_shell_task_is_followed_from_start_to_end() {
        let mut tasks = Tasks::default();
        tasks.note_tool_call(
            "toolu_1",
            Some(&json!({ "command": "npm run dev", "run_in_background": true })),
            &["Command running in background with ID: b1. Output is being written to: /tmp/claude/x/tasks/b1.output"],
        );
        let started = tasks.read(&json!({
            "type": "system", "subtype": "task_started", "task_id": "b1",
            "tool_use_id": "toolu_1", "description": "Start the dev server",
            "task_type": "local_bash"
        }));
        let started = task(&started);
        assert_eq!(started.kind, "shell");
        assert_eq!(started.command.as_deref(), Some("npm run dev"));
        assert_eq!(
            started.output_file.as_deref(),
            Some("/tmp/claude/x/tasks/b1.output")
        );
        assert!(started.can_stop);

        let ended = tasks.read(&json!({
            "type": "system", "subtype": "task_notification", "task_id": "b1",
            "status": "failed", "summary": "exit 1", "output_file": "/tmp/claude/x/tasks/b1.output"
        }));
        assert_eq!(task(&ended).status, TaskStatus::Failed);
        assert_eq!(task(&ended).summary.as_deref(), Some("exit 1"));
    }

    #[test]
    fn subagents_and_housekeeping_are_not_tasks() {
        let mut tasks = Tasks::default();
        for message in [
            json!({ "type": "system", "subtype": "task_started", "task_id": "a1", "task_type": "local_agent" }),
            json!({ "type": "system", "subtype": "task_started", "task_id": "a2", "subagent_type": "Explore" }),
            json!({ "type": "system", "subtype": "task_started", "task_id": "a3", "ambient": true }),
        ] {
            assert!(tasks.read(&message).is_empty(), "{message}");
        }
    }

    #[test]
    fn a_task_missing_from_the_live_set_has_ended() {
        let mut tasks = Tasks::default();
        let added = tasks.read(&json!({
            "type": "system", "subtype": "background_tasks_changed",
            "tasks": [{ "task_id": "m1", "task_type": "local_workflow", "description": "spec" }]
        }));
        assert_eq!(task(&added).kind, "workflow");
        assert!(!task(&added).can_stop);
        let ended = tasks.read(&json!({
            "type": "system", "subtype": "background_tasks_changed", "tasks": []
        }));
        assert_eq!(task(&ended).status, TaskStatus::Completed);
        // Nothing changes the second time.
        assert!(
            tasks
                .read(
                    &json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": [] })
                )
                .is_empty()
        );
    }

    #[test]
    fn activity_follows_the_session_state() {
        let mut tasks = Tasks::default();
        let state =
            |state| json!({ "type": "system", "subtype": "session_state_changed", "state": state });
        assert_eq!(tasks.read(&state("running")), [Report::Working(true)]);
        assert_eq!(tasks.read(&state("idle")), [Report::Working(false)]);
        assert!(tasks.read(&state("requires_action")).is_empty());
    }

    #[test]
    fn output_paths_are_found_in_tool_text() {
        assert_eq!(
            output_paths("Output is being written to: /a/b/tasks/x1.output. Check later."),
            ["/a/b/tasks/x1.output"]
        );
        assert!(output_paths("see notes.output for more").is_empty());
        // Folders can have spaces in them.
        assert_eq!(
            output_paths("written to: /a/my repo/tasks/x2.output"),
            ["/a/my repo/tasks/x2.output"]
        );
    }

    #[test]
    fn only_the_end_of_a_long_output_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.output");
        let line = "0123456789\n".repeat(MAX_TASK_OUTPUT_BYTES / 10);
        std::fs::write(&path, format!("first\n{line}")).unwrap();
        let (text, truncated) = read_output(&path).unwrap();
        assert!(truncated);
        assert!(text.starts_with("0123456789\n"));
        assert!(!text.contains("first"));
        std::fs::write(&path, "short\n").unwrap();
        assert_eq!(read_output(&path).unwrap(), ("short\n".to_owned(), false));
    }

    #[tokio::test]
    async fn stopping_a_task_ends_the_command_writing_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.output");
        // The command runs two levels below its shell, like one Claude Code started, and
        // leads its own group.
        let script = format!(
            "set -m; (sleep 30 > '{}' 2>&1 &) ; sleep 0.2",
            path.display()
        );
        let status = tokio::process::Command::new("sh")
            .args(["-c", &script])
            .status()
            .await
            .unwrap();
        assert!(status.success());
        assert!(stop(&path).await.unwrap() > 0);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(stop(&path).await.unwrap(), 0, "it's still running");
    }
}
