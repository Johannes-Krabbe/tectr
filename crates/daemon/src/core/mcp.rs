//! The daemon's MCP servers: keeping them, logging in to them, and the proxy's view of them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use protocol::{
    CommandError, CommandOutput, DaemonState, McpAuth, McpServer, McpServerId, McpTransport,
    SessionId, SessionStatus, StateChange, Topic, TopicId,
};

use super::{Core, Inner, internal, invalid, not_found};
use crate::mcp::{self, expand};
use crate::oauth;

/// A browser login that takes longer than this has been given up on.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// A login started in a browser and not finished yet.
pub(super) struct PendingLogin {
    server_id: McpServerId,
    pending: oauth::Pending,
    started: Instant,
}

/// Where the proxy sends one session's requests for one server.
pub struct ProxyTarget {
    pub server_id: McpServerId,
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// What a topic's agents would get, apart from logins. A running agent whose topic no longer
/// matches it is out of date.
pub(super) fn fingerprint(state: &DaemonState, topic: &Topic) -> String {
    let servers: Vec<McpServer> = state
        .topic_mcp_servers(topic)
        .map(|server| McpServer {
            auth: McpAuth::None,
            ..server.clone()
        })
        .collect();
    serde_json::to_string(&servers).unwrap_or_default()
}

fn expanded(headers: &std::collections::BTreeMap<String, String>) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| (name.clone(), expand(value)))
        .collect()
}

fn check_transport(transport: &McpTransport) -> Result<(), CommandError> {
    match transport {
        McpTransport::Stdio { command, env, .. } => {
            if command.trim().is_empty() {
                return Err(invalid("a stdio server needs a command"));
            }
            if env.keys().any(|key| key.is_empty() || key.contains('=')) {
                return Err(invalid("an environment variable needs a name without ="));
            }
        }
        McpTransport::Http { url, headers, .. } => {
            let parsed = reqwest::Url::parse(url)
                .map_err(|_| invalid(format!("{url} isn't a valid address")))?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(invalid(
                    "an HTTP server's address starts with http:// or https://",
                ));
            }
            for name in headers.keys() {
                if reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err() {
                    return Err(invalid(format!("{name} isn't a valid header name")));
                }
            }
        }
        McpTransport::Unknown => {
            return Err(invalid("this daemon doesn't know that kind of server"));
        }
    }
    Ok(())
}

fn http_url(transport: &McpTransport) -> Option<&str> {
    match transport {
        McpTransport::Http { url, .. } => Some(url),
        _ => None,
    }
}

impl Core {
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// `localhost`, because that's what servers accept from Claude Code. Browsers that try
    /// IPv6 first fall back to the IPv4 loopback the daemon listens on.
    fn redirect_uri(&self) -> String {
        format!("http://localhost:{}/oauth/callback", self.port)
    }

    pub(super) fn save_mcp_server(
        self: &Arc<Self>,
        id: Option<McpServerId>,
        name: &str,
        topic_id: Option<TopicId>,
        transport: McpTransport,
        enabled: bool,
    ) -> Result<CommandOutput, CommandError> {
        let name = name.trim();
        if !mcp::valid_name(name) {
            return Err(invalid(
                "an MCP server's name needs letters, digits, - or _, and at most 64 of them",
            ));
        }
        check_transport(&transport)?;
        let mut inner = self.lock();
        if let Some(topic_id) = &topic_id
            && inner.state.topic(topic_id).is_none()
        {
            return Err(not_found("topic", topic_id));
        }
        let existing = match &id {
            Some(id) => Some(
                inner
                    .state
                    .mcp_server(id)
                    .cloned()
                    .ok_or_else(|| not_found("MCP server", id))?,
            ),
            None => None,
        };
        if let Some(existing) = &existing
            && existing.topic_id != topic_id
        {
            return Err(invalid(
                "a server can't move between a topic and the whole device. Add a new one instead",
            ));
        }
        // A login belongs to the address it was made for.
        let same_address = existing.as_ref().is_some_and(|e| {
            http_url(&e.transport).is_some() && http_url(&e.transport) == http_url(&transport)
        });
        let server = McpServer {
            id: id.unwrap_or_else(|| McpServerId(crate::new_id())),
            name: name.to_owned(),
            topic_id,
            transport,
            enabled,
            auth: match &existing {
                Some(existing) if same_address => existing.auth,
                _ => McpAuth::None,
            },
        };
        if mcp::name_taken(&inner.state.mcp_servers, &server) {
            return Err(invalid(format!(
                "there's already an MCP server called {} that the same topics would get",
                server.name
            )));
        }
        if existing.is_some() && !same_address {
            inner.store.remove_mcp_login(&server.id).map_err(internal)?;
        }
        inner.put_mcp_server(server.clone()).map_err(internal)?;
        inner.refresh_mcp_sessions(None);
        drop(inner);
        if http_url(&server.transport).is_some() && server.auth != McpAuth::LoggedIn {
            self.check_mcp_auth(server.id.clone());
        }
        Ok(CommandOutput::McpServer { server })
    }

    pub(super) fn remove_mcp_server(
        &self,
        id: &McpServerId,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        if inner.state.mcp_server(id).is_none() {
            return Err(not_found("MCP server", id));
        }
        inner.drop_mcp_server(id).map_err(internal)?;
        inner.refresh_mcp_sessions(None);
        Ok(CommandOutput::Done)
    }

    pub(super) fn set_topic_mcp(
        &self,
        topic_id: &TopicId,
        server_id: &McpServerId,
        on: Option<bool>,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let server = inner
            .state
            .mcp_server(server_id)
            .ok_or_else(|| not_found("MCP server", server_id))?;
        if server.topic_id.is_some() {
            return Err(invalid(
                "a topic's own server is turned on and off with its enabled setting",
            ));
        }
        let topic = inner
            .state
            .topic(topic_id)
            .ok_or_else(|| not_found("topic", topic_id))?;
        let mut overrides = topic.mcp.clone();
        match on {
            Some(on) => overrides.insert(server_id.clone(), on),
            None => overrides.remove(server_id),
        };
        if overrides == topic.mcp {
            return Ok(CommandOutput::Done);
        }
        let candidate = McpServer {
            enabled: true,
            ..server.clone()
        };
        // Turning a device server on can't give a topic two servers with one name.
        if on == Some(true)
            && inner.state.mcp_servers.iter().any(|other| {
                other.topic_id.as_ref() == Some(topic_id) && other.name == candidate.name
            })
        {
            return Err(invalid(format!(
                "this topic has its own server called {}",
                candidate.name
            )));
        }
        inner
            .store
            .set_topic_mcp(topic_id, &overrides)
            .map_err(internal)?;
        inner.emit(StateChange::TopicMcpChanged {
            topic_id: topic_id.clone(),
            mcp: overrides,
        });
        inner.refresh_mcp_sessions(None);
        Ok(CommandOutput::Done)
    }

    pub(super) async fn login_mcp_server(
        &self,
        id: &McpServerId,
    ) -> Result<CommandOutput, CommandError> {
        let (url, headers, client_id, previous) = {
            let inner = self.lock();
            let server = inner
                .state
                .mcp_server(id)
                .ok_or_else(|| not_found("MCP server", id))?;
            let McpTransport::Http {
                url,
                headers,
                oauth_client_id,
            } = &server.transport
            else {
                return Err(invalid("only HTTP servers log in"));
            };
            let previous = inner.store.mcp_login(id).map_err(internal)?;
            (
                expand(url),
                expanded(headers),
                oauth_client_id.clone(),
                previous,
            )
        };
        let state = crate::new_id();
        let (address, pending) = oauth::start(
            &self.http,
            &url,
            &headers,
            client_id.as_deref(),
            previous.as_ref(),
            &self.redirect_uri(),
            &state,
        )
        .await
        .map_err(|error| invalid(format!("{error:#}")))?;
        let mut logins = self.lock_logins();
        logins.retain(|_, login| login.started.elapsed() < LOGIN_TIMEOUT);
        logins.insert(
            state,
            PendingLogin {
                server_id: id.clone(),
                pending,
                started: Instant::now(),
            },
        );
        Ok(CommandOutput::McpLogin { url: address })
    }

    /// Finishes a login with what the server sent back, and returns the server's name.
    pub async fn complete_mcp_login(
        &self,
        state: &str,
        code: &str,
    ) -> Result<String, CommandError> {
        let login = self
            .lock_logins()
            .remove(state)
            .filter(|login| login.started.elapsed() < LOGIN_TIMEOUT)
            .ok_or_else(|| invalid("this login has expired or was already used. Start it again"))?;
        let tokens = oauth::finish(&self.http, login.pending, code)
            .await
            .map_err(|error| invalid(format!("{error:#}")))?;
        let mut inner = self.lock();
        let name = inner
            .state
            .mcp_server(&login.server_id)
            .ok_or_else(|| not_found("MCP server", &login.server_id))?
            .name
            .clone();
        inner
            .store
            .set_mcp_login(&login.server_id, &tokens)
            .map_err(internal)?;
        inner.set_mcp_auth(&login.server_id, McpAuth::LoggedIn);
        // Agents that already gave up on the server only try again when they start.
        inner.refresh_mcp_sessions(Some(&login.server_id));
        Ok(name)
    }

    pub fn cancel_mcp_login(&self, state: &str) {
        self.lock_logins().remove(state);
    }

    pub(super) fn logout_mcp_server(
        &self,
        id: &McpServerId,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let server = inner
            .state
            .mcp_server(id)
            .ok_or_else(|| not_found("MCP server", id))?;
        if http_url(&server.transport).is_none() {
            return Err(invalid("only HTTP servers log in"));
        }
        inner.store.remove_mcp_login(id).map_err(internal)?;
        if server.auth == McpAuth::LoggedIn {
            inner.set_mcp_auth(id, McpAuth::NeedsLogin);
        }
        Ok(CommandOutput::Done)
    }

    pub(super) async fn import_mcp_servers(
        self: &Arc<Self>,
    ) -> Result<CommandOutput, CommandError> {
        let home = self.config_home.clone();
        let (found, mut skipped) = tokio::task::spawn_blocking(move || mcp::import(&home))
            .await
            .map_err(|error| internal(error.into()))?;
        let mut added = vec![];
        let mut to_check = vec![];
        {
            let mut inner = self.lock();
            for found in found {
                let server = McpServer {
                    id: McpServerId(crate::new_id()),
                    name: found.name,
                    topic_id: None,
                    transport: found.transport,
                    enabled: found.enabled,
                    auth: McpAuth::None,
                };
                if mcp::name_taken(&inner.state.mcp_servers, &server) {
                    skipped.push(format!(
                        "{}: there's already a server with this name",
                        server.name
                    ));
                    continue;
                }
                inner.put_mcp_server(server.clone()).map_err(internal)?;
                if http_url(&server.transport).is_some() {
                    to_check.push(server.id.clone());
                }
                added.push(server.name);
            }
            inner.refresh_mcp_sessions(None);
        }
        for id in to_check {
            self.check_mcp_auth(id);
        }
        Ok(CommandOutput::McpImport { added, skipped })
    }

    /// Asks a server whether it wants a login, so the person finds out before an agent does.
    fn check_mcp_auth(self: &Arc<Self>, id: McpServerId) {
        let core = self.clone();
        tokio::spawn(async move {
            let (url, headers) = {
                let inner = core.lock();
                let Some(server) = inner.state.mcp_server(&id) else {
                    return;
                };
                let McpTransport::Http { url, headers, .. } = &server.transport else {
                    return;
                };
                if server.auth == McpAuth::LoggedIn {
                    return;
                }
                (expand(url), expanded(headers))
            };
            match oauth::challenge(&core.http, &url, &headers).await {
                // A login can finish while the server is being asked, and that answer is stale.
                Ok(Some(_)) => {
                    let mut inner = core.lock();
                    let logged_in = inner
                        .store
                        .mcp_login(&id)
                        .ok()
                        .flatten()
                        .is_some_and(|login| login.access_token.is_some());
                    if !logged_in {
                        inner.set_mcp_auth(&id, McpAuth::NeedsLogin);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::debug!(%id, "couldn't ask an MCP server about logins: {error:#}")
                }
            }
        });
    }

    /// Where a request on the proxy goes, if the session behind `secret` got this server.
    pub fn mcp_proxy_target(&self, secret: &str, server_id: &str) -> Option<ProxyTarget> {
        let inner = self.lock();
        let live = inner
            .live
            .values()
            .find(|live| live.proxy_secret == secret)?;
        let server_id = live.mcp.iter().find(|id| id.0 == server_id)?.clone();
        let server = inner.state.mcp_server(&server_id)?;
        let McpTransport::Http { url, headers, .. } = &server.transport else {
            return None;
        };
        Some(ProxyTarget {
            url: expand(url),
            headers: expanded(headers),
            server_id,
        })
    }

    /// The token to send to a server, refreshed first if it's about to expire, or if `force`
    /// says the server just refused it.
    pub async fn mcp_access_token(&self, id: &McpServerId, force: bool) -> Option<String> {
        let stored = || self.lock().store.mcp_login(id).ok().flatten();
        let refused = stored()?.access_token?;
        let usable = |login: &oauth::Login| {
            let token = login.access_token.clone()?;
            let fresh = !login.is_stale(oauth::now_ms());
            // Another request may have refreshed it while this one waited.
            (fresh && (!force || token != refused)).then_some(token)
        };
        if let Some(token) = stored().as_ref().and_then(usable) {
            return Some(token);
        }
        let _one_at_a_time = self.token_refresh.lock().await;
        let login = stored()?;
        if let Some(token) = usable(&login) {
            return Some(token);
        }
        match oauth::refresh(&self.http, &login).await {
            Ok(refreshed) => {
                let token = refreshed.access_token.clone();
                let inner = self.lock();
                if let Err(error) = inner.store.set_mcp_login(id, &refreshed) {
                    tracing::error!(%id, "failed to store a refreshed login: {error:#}");
                }
                token
            }
            Err(oauth::RefreshError::Expired) => {
                let mut inner = self.lock();
                let signed_out = oauth::Login {
                    access_token: None,
                    refresh_token: None,
                    expires_at_ms: None,
                    ..login
                };
                if let Err(error) = inner.store.set_mcp_login(id, &signed_out) {
                    tracing::error!(%id, "failed to store an expired login: {error:#}");
                }
                inner.set_mcp_auth(id, McpAuth::NeedsLogin);
                None
            }
            Err(oauth::RefreshError::Failed(message)) => {
                tracing::warn!(%id, "couldn't refresh an MCP login: {message}");
                // It may still work until it expires.
                (!force).then_some(login.access_token).flatten()
            }
        }
    }

    pub fn mcp_needs_login(&self, id: &McpServerId) {
        self.lock().set_mcp_auth(id, McpAuth::NeedsLogin);
    }

    fn lock_logins(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, PendingLogin>> {
        self.logins
            .lock()
            .expect("a panic poisoned the pending logins")
    }
}

impl Inner {
    pub(super) fn put_mcp_server(&mut self, server: McpServer) -> anyhow::Result<()> {
        self.store.upsert_mcp_server(&server)?;
        self.emit(StateChange::McpServerChanged { server });
        Ok(())
    }

    pub(super) fn drop_mcp_server(&mut self, id: &McpServerId) -> anyhow::Result<()> {
        self.store.remove_mcp_server(id)?;
        self.emit(StateChange::McpServerRemoved { id: id.clone() });
        Ok(())
    }

    fn set_mcp_auth(&mut self, id: &McpServerId, auth: McpAuth) {
        let Some(server) = self.state.mcp_server(id) else {
            return;
        };
        if server.auth == auth {
            return;
        }
        let server = McpServer {
            auth,
            ..server.clone()
        };
        if let Err(error) = self.put_mcp_server(server) {
            tracing::error!(%id, "failed to store an MCP login state: {error:#}");
        }
    }

    /// Stops the agents whose MCP servers changed, so their next prompt starts them with the
    /// new ones. ACP can't change a running session's servers. An agent that's working or
    /// waiting finishes first. `login` also counts agents that get that server.
    pub(super) fn refresh_mcp_sessions(&mut self, login: Option<&McpServerId>) {
        let mut outdated: Vec<SessionId> = vec![];
        for topic in self.state.topics() {
            let current = fingerprint(&self.state, topic);
            for session in &topic.sessions {
                if let Some(live) = self.live.get(&session.id)
                    && (live.mcp_fingerprint != current
                        || login.is_some_and(|id| live.mcp.contains(id)))
                {
                    outdated.push(session.id.clone());
                }
            }
        }
        for id in outdated {
            let Some(live) = self.live.get_mut(&id) else {
                continue;
            };
            if live.turn_active || !live.pending.is_empty() {
                live.mcp_stale = true;
                continue;
            }
            self.stop_for_mcp(&id);
        }
    }

    pub(super) fn stop_for_mcp(&mut self, id: &SessionId) {
        if let Some(task) = self.stop_agent(id) {
            self.keep_closing(task);
        }
        let status = self.state.session(id).map(|session| session.status);
        if matches!(
            status,
            Some(SessionStatus::Starting | SessionStatus::Ready | SessionStatus::Done)
        ) {
            self.set_status(id, SessionStatus::Stopped);
        }
    }
}
