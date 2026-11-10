//! Bringing in conversations Claude Code and Codex keep themselves. Reading them is in
//! `crate::import`.

use protocol::{
    CommandError, CommandOutput, PermissionMode, Session, SessionEvent, SessionEventKind,
    SessionId, SessionRef, SessionStatus, StateChange, TopicId,
};

use super::{Core, internal, invalid, not_found, now_ms};
use crate::import::{self, Source};

impl Core {
    /// Only agents added to this device, because their conversations are the ones it can carry on.
    fn import_sources(&self) -> Vec<Source> {
        let inner = self.lock();
        Source::ALL
            .into_iter()
            .filter(|source| {
                self.agents.contains_key(source.agent())
                    && inner
                        .state
                        .providers
                        .iter()
                        .any(|p| p.agent == source.agent())
            })
            .collect()
    }

    pub(super) async fn list_conversations(&self) -> Result<CommandOutput, CommandError> {
        let sources = self.import_sources();
        let home = self.config_home.clone();
        let mut conversations = tokio::task::spawn_blocking(move || import::list(&home, &sources))
            .await
            .map_err(|error| internal(error.into()))?;
        let open = self.lock().store.open_agent_sessions().map_err(internal)?;
        conversations.retain(|c| !open.contains(&(c.agent.clone(), c.id.clone())));
        Ok(CommandOutput::Conversations { conversations })
    }

    pub(super) async fn import_conversation(
        &self,
        topic_id: TopicId,
        agent: String,
        id: String,
    ) -> Result<CommandOutput, CommandError> {
        let source = Source::of_agent(&agent)
            .filter(|source| self.import_sources().contains(source))
            .ok_or_else(|| {
                invalid(format!(
                    "this device can't import conversations for {agent}"
                ))
            })?;
        {
            let inner = self.lock();
            if inner.state.topic(&topic_id).is_none() {
                return Err(not_found("topic", &topic_id));
            }
            let open = inner.store.open_agent_sessions().map_err(internal)?;
            if open.contains(&(agent.clone(), id.clone())) {
                return Err(invalid("that conversation is already open in a tab"));
            }
        }

        let home = self.config_home.clone();
        let wanted = id.clone();
        let imported = tokio::task::spawn_blocking(move || import::load(&home, source, &wanted))
            .await
            .map_err(|error| internal(error.into()))?
            .map_err(|error| invalid(format!("{error:#}")))?;

        let mut inner = self.lock();
        // It may have been archived while the conversation was read.
        if inner.state.topic(&topic_id).is_none() {
            return Err(not_found("topic", &topic_id));
        }
        let provider = inner
            .state
            .providers
            .iter()
            .find(|p| p.agent == agent)
            .ok_or_else(|| invalid(format!("{agent} was removed from this device")))?;
        // It starts like a new tab would, with the provider's defaults and tectr's own mode.
        let mode = if provider.modes.contains(&PermissionMode::Auto) {
            PermissionMode::Auto
        } else {
            *provider
                .modes
                .first()
                .ok_or_else(|| invalid(format!("{agent} has no permission mode tectr can use")))?
        };
        let session = Session {
            id: SessionId(crate::new_id()),
            agent,
            mode,
            model: provider.default_model.clone(),
            effort: provider.default_effort.clone(),
            status: SessionStatus::Stopped,
            title: Some(imported.title.clone()),
            unread: false,
            opened_at_ms: now_ms(),
        };
        let reference = SessionRef {
            machine_id: inner.machine_id().clone(),
            session_id: session.id.clone(),
        };
        let stopped = SessionEventKind::StatusChanged {
            status: SessionStatus::Stopped,
        };
        let events: Vec<SessionEvent> = imported
            .events
            .into_iter()
            .chain([(now_ms(), stopped)])
            .zip(1..)
            .map(|((at_ms, kind), seq)| SessionEvent {
                session: reference.clone(),
                seq,
                at_ms,
                kind,
            })
            .collect();
        // Stored before anyone hears of the tab, so following it reads the whole history.
        inner
            .store
            .import_session(&topic_id, &session, &id, &imported.cwd, &events)
            .map_err(internal)?;
        inner
            .last_seq
            .insert(session.id.clone(), events.len() as u64);
        inner.emit(StateChange::SessionAdded {
            topic_id: topic_id.clone(),
            session: session.clone(),
        });
        inner.name_topic_after(&session.id, imported.title.trim());
        Ok(CommandOutput::Session { topic_id, session })
    }
}
