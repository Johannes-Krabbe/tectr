//! Branching a tab: a new tab in the same topic with the history up to one of its answers,
//! whose agent copies its own session up to there, so both can carry on separately.

use std::sync::Arc;

use protocol::{
    CommandError, CommandOutput, Session, SessionEvent, SessionEventKind, SessionId, SessionRef,
    SessionStatus, StateChange,
};

use super::{Core, internal, invalid, not_found, now_ms};
use crate::acp::ForkPoint;

const BRANCH_SUFFIX: &str = " (branch)";

impl Core {
    pub(super) fn fork_session(
        self: &Arc<Self>,
        source_id: &SessionId,
        after_seq: Option<u64>,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let source = inner
            .state
            .session(source_id)
            .ok_or_else(|| not_found("session", source_id))?
            .clone();
        let topic_id = inner
            .state
            .topics()
            .find(|topic| topic.sessions.iter().any(|s| &s.id == source_id))
            .map(|topic| topic.id.clone())
            .ok_or_else(|| not_found("session", source_id))?;
        let provider = inner
            .state
            .providers
            .iter()
            .find(|p| p.agent == source.agent)
            .ok_or_else(|| invalid(format!("{} was removed from this device", source.agent)))?;
        if !provider.can_fork {
            return Err(invalid(format!("{} can't branch sessions", provider.name)));
        }
        let agent_session_id = inner
            .store
            .agent_session_id(source_id)
            .map_err(internal)?
            .ok_or_else(|| invalid("this tab's agent hasn't started a session to branch from"))?;
        let seq = match after_seq {
            Some(seq) => seq,
            None => inner
                .store
                .last_turn_end(source_id)
                .map_err(internal)?
                .ok_or_else(|| invalid("this tab has no finished answer to branch from"))?,
        };
        let machine_id = inner.machine_id().clone();
        let history = inner
            .store
            .events_through(&machine_id, source_id, seq)
            .map_err(internal)?;
        let message_id = match history.last() {
            Some(SessionEvent {
                seq: last,
                kind: SessionEventKind::TurnEnded { last_message, .. },
                ..
            }) if *last == seq => last_message.clone().ok_or_else(|| {
                invalid("the agent didn't say where that answer ends, so it can't be branched")
            })?,
            _ => return Err(invalid("that isn't the end of an answer")),
        };
        let agent_cwd = match inner.store.agent_cwd(source_id).map_err(internal)? {
            Some(cwd) => cwd,
            None => inner
                .folder_of(source_id)
                .ok_or_else(|| internal(anyhow::anyhow!("the session's topic has no folder")))?
                .path
                .clone(),
        };

        let session = Session {
            id: SessionId(crate::new_id()),
            agent: source.agent.clone(),
            mode: source.mode,
            model: source.model.clone(),
            effort: source.effort.clone(),
            status: SessionStatus::Starting,
            title: source.title.as_ref().map(|title| {
                let title = title.strip_suffix(BRANCH_SUFFIX).unwrap_or(title);
                format!("{title}{BRANCH_SUFFIX}")
            }),
            unread: false,
            // Tabs go in the order they opened, so this puts it right after the one it came from.
            opened_at_ms: source.opened_at_ms + 1,
        };
        let reference = SessionRef {
            machine_id,
            session_id: session.id.clone(),
        };
        let starting = SessionEventKind::StatusChanged {
            status: SessionStatus::Starting,
        };
        let events: Vec<SessionEvent> = history
            .into_iter()
            // Those were about the source's agent process, not this one's.
            .filter(|event| {
                !matches!(
                    event.kind,
                    SessionEventKind::StatusChanged { .. } | SessionEventKind::AgentReady { .. }
                )
            })
            .map(|event| (event.at_ms, event.kind))
            .chain([(now_ms(), starting)])
            .zip(1..)
            .map(|((at_ms, kind), seq)| SessionEvent {
                session: reference.clone(),
                seq,
                at_ms,
                kind,
            })
            .collect();
        let fork = ForkPoint {
            agent_session_id,
            message_id,
        };
        // Stored before anyone hears of the tab, so following it reads the whole history.
        inner
            .store
            .insert_branch(&topic_id, &session, &fork, &agent_cwd, &events)
            .map_err(internal)?;
        inner
            .last_seq
            .insert(session.id.clone(), events.len() as u64);
        inner.emit(StateChange::SessionAdded {
            topic_id: topic_id.clone(),
            session: session.clone(),
        });
        self.spawn_agent(&mut inner, &session, false)
            .map_err(internal)?;
        Ok(CommandOutput::Session { topic_id, session })
    }
}
