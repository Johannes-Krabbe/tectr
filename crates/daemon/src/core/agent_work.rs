//! What an agent does besides answering: asking questions, starting subagents and background
//! tasks, changing its own mode and options, working on turns it starts itself. And what the
//! person can do about it: answer, stop, read output, set options, mention files and run
//! commands of their own.

use std::time::Duration;

use protocol::{
    AgentOption, CommandError, CommandOutput, NoticeSeverity, PermissionMode, Question,
    QuestionAnswer, QuestionField, QuestionId, SessionEventKind, SessionId, SessionStatus,
    StateChange, StopReason, TaskStatus,
};
use tokio::sync::oneshot;

use super::{Core, Inner, Prompt, QuestionResponder, SessionCommand, internal, invalid, not_found};

/// How long a `!` command may run before it's stopped.
const SHELL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The most of a `!` command's output that's kept, from its end.
const MAX_SHELL_OUTPUT: usize = 30_000;

impl Core {
    pub fn question_asked(
        &self,
        id: &SessionId,
        subagent: Option<String>,
        message: String,
        fields: Vec<QuestionField>,
        url: Option<String>,
        answer: QuestionResponder,
    ) {
        let mut inner = self.lock();
        let question = Question {
            id: QuestionId(crate::new_id()),
            session_id: id.clone(),
            subagent,
            message,
            fields,
            url,
        };
        let Some(live) = inner.live.get_mut(id) else {
            answer(None);
            return;
        };
        live.questions.insert(question.id.clone(), answer);
        if let Err(error) = inner.store.insert_pending_question(&question) {
            tracing::error!(%id, "failed to store a pending question: {error:#}");
        }
        inner.append(
            id,
            SessionEventKind::QuestionAsked {
                question: question.clone(),
            },
        );
        inner.emit(StateChange::QuestionAdded { question });
        inner.set_status(id, SessionStatus::WaitingForYou);
    }

    pub(super) fn answer_question(
        &self,
        question_id: &QuestionId,
        answer: QuestionAnswer,
    ) -> Result<CommandOutput, CommandError> {
        if matches!(answer, QuestionAnswer::Unknown) {
            return Err(invalid("that isn't an answer this daemon knows"));
        }
        let mut inner = self.lock();
        let question = inner
            .state
            .questions
            .iter()
            .find(|question| &question.id == question_id)
            .ok_or_else(|| not_found("pending question", question_id))?
            .clone();
        let session_id = question.session_id.clone();
        let live = inner
            .live
            .get_mut(&session_id)
            .ok_or_else(|| not_found("running session for question", question_id))?;
        let respond = live
            .questions
            .remove(question_id)
            .ok_or_else(|| not_found("pending question", question_id))?;
        let others_waiting = !live.pending.is_empty() || !live.questions.is_empty();
        let working = live.turn_active || live.autonomous;
        let (content, note) = match &answer {
            QuestionAnswer::Accept { values, note } => (
                crate::questions::content(&question.fields, values),
                note.as_deref()
                    .map(str::trim)
                    .filter(|note| !note.is_empty())
                    .map(str::to_owned),
            ),
            _ => (serde_json::Map::new(), None),
        };
        let recorded = match &answer {
            QuestionAnswer::Accept { .. } => QuestionAnswer::Accept {
                values: content.clone(),
                note: note.clone(),
            },
            other => other.clone(),
        };
        respond(Some((answer, content)));
        inner.resolve_question(&session_id, question_id, recorded);
        if !others_waiting {
            let status = if working {
                SessionStatus::Working
            } else {
                SessionStatus::Done
            };
            inner.set_status(&session_id, status);
        }
        if let Some(text) = note {
            inner.append(
                &session_id,
                SessionEventKind::UserPrompt {
                    text: text.clone(),
                    images: vec![],
                },
            );
            // Into the turn the question belongs to.
            inner.tell_agent(&session_id, text);
        }
        Ok(CommandOutput::Done)
    }

    pub(super) fn stop_subagent(
        &self,
        session_id: &SessionId,
        subagent: String,
    ) -> Result<CommandOutput, CommandError> {
        let inner = self.lock();
        let live = inner
            .live
            .get(session_id)
            .ok_or_else(|| invalid("this session's agent isn't running"))?;
        live.commands
            .send(SessionCommand::CancelSubagent(subagent))
            .map_err(|_| internal(anyhow::anyhow!("the agent has just exited")))?;
        Ok(CommandOutput::Done)
    }

    /// The task as its latest event describes it, from the session's history.
    fn task(
        &self,
        session_id: &SessionId,
        task_id: &str,
    ) -> Result<protocol::BackgroundTask, CommandError> {
        let inner = self.lock();
        let machine_id = inner.machine_id().clone();
        let events = inner
            .store
            .events_after(&machine_id, session_id, 0, usize::MAX >> 1)
            .map_err(internal)?;
        events
            .into_iter()
            .rev()
            .find_map(|event| match event.kind {
                SessionEventKind::Task { task } if task.id == task_id => Some(task),
                _ => None,
            })
            .ok_or_else(|| not_found("background task", &task_id))
    }

    pub(super) async fn stop_task(
        &self,
        session_id: &SessionId,
        task_id: &str,
    ) -> Result<CommandOutput, CommandError> {
        let task = self.task(session_id, task_id)?;
        if task.status != TaskStatus::Running {
            return Err(invalid("this task isn't running"));
        }
        let path = task
            .output_file
            .as_deref()
            .filter(|_| task.can_stop)
            .ok_or_else(|| invalid("tectr can't stop this kind of task"))?;
        let stopped = crate::tasks::stop(std::path::Path::new(path))
            .await
            .map_err(|error| invalid(format!("{error:#}")))?;
        if stopped == 0 {
            return Err(invalid("nothing is running for this task anymore"));
        }
        self.tell_task_stopped(session_id, &task);
        Ok(CommandOutput::Done)
    }

    /// Stopping a task kills its command, which the agent hears about as the command failing,
    /// like Claude Code's exit code 144. So the person's stop is said to it as theirs, in the
    /// turn that hears about it, and shown in the transcript as a notice.
    fn tell_task_stopped(&self, session_id: &SessionId, task: &protocol::BackgroundTask) {
        let mut inner = self.lock();
        let name = if task.description.is_empty() {
            task.command.as_deref().unwrap_or(&task.kind)
        } else {
            &task.description
        };
        let command = task
            .command
            .as_deref()
            .filter(|command| *command != name)
            .map(|command| format!(" (`{command}`)"))
            .unwrap_or_default();
        inner.append(
            session_id,
            SessionEventKind::Notice {
                severity: NoticeSeverity::Info,
                title: format!("You stopped {name}"),
                description: None,
            },
        );
        inner.tell_agent(
            session_id,
            format!(
                "I stopped the background task \"{name}\"{command} (ID {}) myself, so it ending now is \
                 because of that and isn't a failure.",
                task.id
            ),
        );
    }

    /// A background task's latest report, which gives it a tab while it runs and keeps one
    /// that failed, so the reason can be read. One that finished or was stopped goes, since
    /// the agent says how it went.
    pub fn task_changed(&self, id: &SessionId, task: protocol::BackgroundTask) {
        let mut inner = self.lock();
        if inner.state.session(id).is_none()
            || inner
                .live
                .get(id)
                .is_some_and(|live| live.closed_tasks.contains(&task.id))
        {
            return;
        }
        let before = inner
            .state
            .session_tasks
            .get(id)
            .cloned()
            .unwrap_or_default();
        let mut tasks = before.clone();
        let keep = matches!(task.status, TaskStatus::Running | TaskStatus::Failed);
        match (tasks.iter().position(|t| t.id == task.id), keep) {
            (Some(at), true) => tasks[at] = task,
            (None, true) => tasks.push(task),
            (Some(at), false) => {
                tasks.remove(at);
            }
            (None, false) => {}
        }
        if tasks != before {
            inner.emit(StateChange::SessionTasksChanged {
                session_id: id.clone(),
                tasks,
            });
        }
    }

    /// Closes a background task's tab, stopping the task first if it's still running. A task
    /// that turns out to have ended already closes all the same.
    pub(super) async fn close_task(
        &self,
        session_id: &SessionId,
        task_id: &str,
    ) -> Result<CommandOutput, CommandError> {
        let task = {
            let mut inner = self.lock();
            let Some(task) = inner
                .state
                .session_tasks
                .get(session_id)
                .and_then(|tasks| tasks.iter().find(|t| t.id == task_id))
                .cloned()
            else {
                return Err(not_found("background task", &task_id));
            };
            if let Some(live) = inner.live.get_mut(session_id) {
                live.closed_tasks.insert(task_id.to_owned());
            }
            let tasks = inner.state.session_tasks[session_id]
                .iter()
                .filter(|t| t.id != task_id)
                .cloned()
                .collect();
            inner.emit(StateChange::SessionTasksChanged {
                session_id: session_id.clone(),
                tasks,
            });
            task
        };
        if task.status == TaskStatus::Running
            && task.can_stop
            && let Some(path) = &task.output_file
        {
            let stopped = crate::tasks::stop(std::path::Path::new(path))
                .await
                .map_err(|error| invalid(format!("{error:#}")))?;
            if stopped > 0 {
                self.tell_task_stopped(session_id, &task);
            }
        }
        Ok(CommandOutput::Done)
    }

    pub(super) async fn read_task_output(
        &self,
        session_id: &SessionId,
        task_id: &str,
    ) -> Result<CommandOutput, CommandError> {
        let task = self.task(session_id, task_id)?;
        let path = task
            .output_file
            .ok_or_else(|| invalid("this task hasn't said where its output goes"))?;
        let (text, truncated) = tokio::task::spawn_blocking(move || {
            crate::tasks::read_output(std::path::Path::new(&path))
        })
        .await
        .map_err(|error| internal(error.into()))?
        .map_err(|error| invalid(format!("{error:#}")))?;
        Ok(CommandOutput::TaskOutput { text, truncated })
    }

    /// The agent moved to another mode by itself, as Claude does once a plan is approved.
    pub fn mode_changed(&self, id: &SessionId, mode: PermissionMode) {
        let mut inner = self.lock();
        let Some(session) = inner.state.session_mut(id) else {
            return;
        };
        if session.mode == mode {
            return;
        }
        session.mode = mode;
        let session = session.clone();
        if let Err(error) = inner.store.update_session(&session) {
            tracing::error!(%id, "failed to store the session's mode: {error:#}");
        }
        inner.emit(StateChange::SessionChanged { session });
    }

    pub fn options_changed(&self, id: &SessionId, options: Vec<AgentOption>) {
        let mut inner = self.lock();
        if inner.state.session(id).is_none()
            || inner
                .state
                .session_options
                .get(id)
                .map_or(&[][..], Vec::as_slice)
                == options.as_slice()
        {
            return;
        }
        inner.emit(StateChange::SessionOptionsChanged {
            session_id: id.clone(),
            options,
        });
    }

    pub(super) async fn set_session_option(
        &self,
        session_id: &SessionId,
        option_id: String,
        value: String,
    ) -> Result<CommandOutput, CommandError> {
        let reply = {
            let inner = self.lock();
            if inner.state.session(session_id).is_none() {
                return Err(not_found("session", session_id));
            }
            let offered = inner
                .state
                .session_options
                .get(session_id)
                .and_then(|options| options.iter().find(|o| o.id == option_id));
            if let Some(option) = offered
                && !option.choices.iter().any(|choice| choice.value == value)
            {
                return Err(invalid(format!("{} has no choice {value}", option.name)));
            }
            inner.live.get(session_id).and_then(|live| {
                let (reply, answer) = oneshot::channel();
                let command = SessionCommand::SetOption {
                    option_id: option_id.clone(),
                    value: value.clone(),
                    reply,
                };
                live.commands.send(command).ok().map(|()| answer)
            })
        };
        if let Some(answer) = reply
            && let Ok(result) = answer.await
        {
            result.map_err(invalid)?;
        }
        let inner = self.lock();
        let mut options = inner.store.session_options(session_id).map_err(internal)?;
        options.insert(option_id.clone(), value.clone());
        inner
            .store
            .set_session_options(session_id, &options)
            .map_err(internal)?;
        drop(inner);
        // A stopped agent says nothing, so what's shown follows the choice until it starts.
        let mut inner = self.lock();
        if !inner.live.contains_key(session_id)
            && let Some(list) = inner.state.session_options.get(session_id).cloned()
        {
            let list = list
                .into_iter()
                .map(|mut option| {
                    if option.id == option_id {
                        option.value.clone_from(&value);
                    }
                    option
                })
                .collect();
            inner.emit(StateChange::SessionOptionsChanged {
                session_id: session_id.clone(),
                options: list,
            });
        }
        Ok(CommandOutput::Done)
    }

    /// Claude Code says when it starts and stops working, including on turns it starts
    /// itself, such as after a background task finished. Those have no prompt, so they'd
    /// otherwise look finished while the agent works.
    pub fn agent_working(&self, id: &SessionId, working: bool, last_message: Option<String>) {
        let mut inner = self.lock();
        let Some(live) = inner.live.get_mut(id) else {
            return;
        };
        live.agent_running = working;
        if live.turn_active {
            return;
        }
        match (working, live.autonomous) {
            (true, false) => {
                live.autonomous = true;
                inner.set_status(id, SessionStatus::Working);
            }
            (false, true) => {
                live.autonomous = false;
                let waiting = !live.pending.is_empty() || !live.questions.is_empty();
                inner.append(
                    id,
                    SessionEventKind::TurnEnded {
                        stop_reason: StopReason::EndTurn,
                        last_message,
                    },
                );
                let status = if waiting {
                    SessionStatus::WaitingForYou
                } else {
                    SessionStatus::Done
                };
                inner.set_status(id, status);
            }
            _ => {}
        }
    }

    /// A message meant for a running turn found none, so it starts a turn of its own: now,
    /// or once this daemon hears the turn it was meant for end.
    pub fn steer_missed(&self, id: &SessionId, prompt: Prompt) {
        let mut inner = self.lock();
        let Some(live) = inner.live.get_mut(id) else {
            return;
        };
        if live.turn_active {
            live.queue(prompt);
            return;
        }
        live.turn_active = true;
        live.autonomous = false;
        if live.commands.send(SessionCommand::Prompt(prompt)).is_ok() {
            inner.set_status(id, SessionStatus::Working);
        }
    }

    pub(super) async fn list_files(
        &self,
        session_id: &SessionId,
        query: &str,
    ) -> Result<CommandOutput, CommandError> {
        let folder = self.folder(session_id)?;
        let paths = crate::mentions::list(std::path::Path::new(&folder), query)
            .await
            .map_err(internal)?;
        Ok(CommandOutput::Files { paths })
    }

    fn folder(&self, session_id: &SessionId) -> Result<String, CommandError> {
        let inner = self.lock();
        if inner.state.session(session_id).is_none() {
            return Err(not_found("session", session_id));
        }
        Ok(inner
            .folder_of(session_id)
            .ok_or_else(|| internal(anyhow::anyhow!("the session's topic has no folder")))?
            .path
            .clone())
    }

    /// Runs a command the person typed with `!`, in the session's folder. What it printed
    /// goes in the transcript, and to the agent with the next prompt.
    pub(super) async fn run_shell(
        &self,
        session_id: &SessionId,
        line: String,
    ) -> Result<CommandOutput, CommandError> {
        let line = line.trim().to_owned();
        if line.is_empty() {
            return Err(invalid("there's no command to run"));
        }
        let folder = self.folder(session_id)?;
        let ran = tokio::time::timeout(
            SHELL_TIMEOUT,
            tokio::process::Command::new(&self.shell.program)
                .arg("-c")
                .arg(&line)
                .current_dir(&folder)
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await;
        let (output, exit_code) = match ran {
            Ok(Ok(output)) => {
                let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&output.stderr));
                (text, output.status.code())
            }
            Ok(Err(error)) => (format!("couldn't run it: {error}"), None),
            Err(_) => ("stopped after ten minutes".to_owned(), None),
        };
        let output = tail(&output, MAX_SHELL_OUTPUT);
        let mut inner = self.lock();
        if inner.state.session(session_id).is_none() {
            return Err(not_found("session", session_id));
        }
        inner
            .shell_context
            .entry(session_id.clone())
            .or_default()
            .push(format!(
                "$ {line}\n{}{}",
                output.trim_end(),
                exit_code.map_or_else(String::new, |code| if code == 0 {
                    String::new()
                } else {
                    format!("\n(exit code {code})")
                })
            ));
        inner.append(
            session_id,
            SessionEventKind::Shell {
                command: line,
                output,
                exit_code,
            },
        );
        Ok(CommandOutput::Done)
    }
}

/// The end of `text`, at most `limit` bytes, starting on a whole character.
fn tail(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("...\n{}", &text[start..])
}

impl Inner {
    /// What was held back while the agent compacted goes into the turn once it's done, or
    /// after the turn if it's ending, which a missed steer takes care of.
    pub(super) fn compaction_changed(&mut self, id: &SessionId, compacting: bool) {
        let Some(live) = self.live.get_mut(id) else {
            return;
        };
        live.compacting = compacting;
        if compacting || !live.turn_active || !live.steering {
            return;
        }
        if let Some(prompt) = live.queued.take() {
            let _ = live.commands.send(SessionCommand::Steer(prompt));
        }
    }

    pub(super) fn resolve_question(
        &mut self,
        session_id: &SessionId,
        question_id: &QuestionId,
        answer: QuestionAnswer,
    ) {
        if let Err(error) = self.store.delete_pending_question(question_id) {
            tracing::error!(%question_id, "failed to delete a pending question: {error:#}");
        }
        self.append(
            session_id,
            SessionEventKind::QuestionAnswered {
                question_id: question_id.clone(),
                answer,
            },
        );
        self.emit(StateChange::QuestionResolved {
            question_id: question_id.clone(),
        });
    }
}

impl Inner {
    /// Says something to a session's agent on the person's behalf: into the turn that's
    /// running, or after it for an agent that can't take that, or as a turn of its own when
    /// none is running.
    fn tell_agent(&mut self, session_id: &SessionId, text: String) {
        let prompt = Prompt {
            text,
            images: vec![],
        };
        let started = self.live.get_mut(session_id).is_some_and(|live| {
            if live.turn_active || live.autonomous {
                if live.steering && !live.compacting {
                    let _ = live.commands.send(SessionCommand::Steer(prompt));
                } else {
                    live.queue(prompt);
                }
                false
            } else {
                live.turn_active = true;
                live.commands.send(SessionCommand::Prompt(prompt)).is_ok()
            }
        });
        if started {
            self.set_status(session_id, SessionStatus::Working);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tail;

    #[test]
    fn a_long_output_keeps_its_end() {
        assert_eq!(tail("short", 10), "short");
        assert_eq!(tail("abcdefghij", 4), "...\nghij");
        // Never splits a character.
        assert_eq!(tail("aééé", 3), "...\né");
    }
}
