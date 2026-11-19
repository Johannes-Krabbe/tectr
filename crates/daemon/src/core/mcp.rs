//! The daemon's MCP servers: keeping them, logging in to them, and the proxy's view of them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use std::collections::BTreeMap;

use protocol::{
    CommandError, CommandOutput, DaemonState, McpAuth, McpProfile, McpProfileId, McpServer,
    McpServerId, McpTransport, SessionId, SessionStatus, StateChange, Topic, WorkspaceId,
    WorkspaceMcp,
};

use super::{Core, Inner, internal, invalid, not_found};
use crate::agents::AgentLauncher;
use crate::mcp::{self, ConnectorKind, expand};
use crate::oauth;

/// How long saving an HTTP server waits to find out whether it wants a login.
const AUTH_CHECK_WAIT: Duration = Duration::from_secs(5);

/// A browser login that takes longer than this has been given up on.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// A login started in a browser and not finished yet.
pub(super) struct PendingLogin {
    server_id: McpServerId,
    profile_id: McpProfileId,
    pending: oauth::Pending,
    started: Instant,
}

/// Where the proxy sends one session's requests for one server.
pub struct ProxyTarget {
    pub server_id: McpServerId,
    /// Whose login goes with the requests.
    pub profile_id: Option<McpProfileId>,
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// What a topic's agents would get, apart from logins. A running agent whose topic no longer
/// matches it is out of date.
pub(super) fn fingerprint(state: &DaemonState, topic: &Topic) -> String {
    let Some(workspace) = state.topic_workspace(&topic.id) else {
        return String::new();
    };
    type Entry<'a> = (
        &'a str,
        &'a McpTransport,
        Option<(&'a McpProfileId, &'a BTreeMap<String, String>)>,
    );
    let servers: Vec<Entry> = state
        .workspace_mcp_servers(workspace)
        .map(|(server, profile)| {
            (
                server.name.as_str(),
                &server.transport,
                profile.map(|p| (&p.id, &p.values)),
            )
        })
        .collect();
    serde_json::to_string(&servers).unwrap_or_default()
}

/// A server's headers with a profile's laid over them, filled in from the environment.
fn expanded(
    headers: &BTreeMap<String, String>,
    profile: Option<&McpProfile>,
) -> Vec<(String, String)> {
    let mut merged = headers.clone();
    if let Some(profile) = profile {
        merged.extend(profile.values.clone());
    }
    merged
        .iter()
        .map(|(name, value)| (name.clone(), expand(value)))
        .collect()
}

/// Profile values have to make sense as what they're laid over.
fn check_values(
    transport: &McpTransport,
    values: &BTreeMap<String, String>,
) -> Result<(), CommandError> {
    match transport {
        McpTransport::Stdio { .. } => {
            if values.keys().any(|key| key.is_empty() || key.contains('=')) {
                return Err(invalid("an environment variable needs a name without ="));
            }
        }
        McpTransport::Http { .. } => {
            for name in values.keys() {
                if reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err() {
                    return Err(invalid(format!("{name} isn't a valid header name")));
                }
            }
        }
        _ => {
            return Err(invalid(
                "connectors have no profiles, their agent holds the login",
            ));
        }
    }
    Ok(())
}

/// A server that turns the daemon away needs a login for each profile that has none.
fn needing_login(server: McpServer) -> McpServer {
    McpServer {
        needs_login: true,
        profiles: server
            .profiles
            .into_iter()
            .map(|profile| McpProfile {
                auth: match profile.auth {
                    McpAuth::LoggedIn => McpAuth::LoggedIn,
                    _ => McpAuth::NeedsLogin,
                },
                ..profile
            })
            .collect(),
        ..server
    }
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
        McpTransport::ClaudeConnector { .. } | McpTransport::CodexApp { .. } => {
            return Err(invalid(
                "connectors come from refreshing them, not from adding them by hand",
            ));
        }
        McpTransport::Unknown => {
            return Err(invalid("this daemon doesn't know that kind of server"));
        }
    }
    Ok(())
}

/// How long `claude mcp list` may take. It checks every server Claude Code has, connectors
/// included, which takes a few seconds.
const LIST_TIMEOUT: Duration = Duration::from_secs(60);

/// Asks Claude Code which claude.ai connectors the person has, with their addresses.
async fn list_claude_connectors(
    claude: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<Vec<mcp::Listed>, String> {
    let output = tokio::process::Command::new(claude)
        .args(["mcp", "list"])
        // An empty folder, so no project's servers get started for nothing.
        .current_dir(cwd)
        .env_remove("ENABLE_CLAUDEAI_MCP_SERVERS")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(LIST_TIMEOUT, output)
        .await
        .map_err(|_| "Claude Code took too long to list its connectors".to_owned())?
        .map_err(|error| format!("couldn't run Claude Code to list its connectors: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Claude Code couldn't list its connectors: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(mcp::claude_connectors(&String::from_utf8_lossy(
        &output.stdout,
    )))
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

    /// Waits briefly for whether an HTTP server wants a login, so the answer can say so and a
    /// client can open the browser straight away.
    pub(super) async fn save_mcp_server(
        self: &Arc<Self>,
        id: Option<McpServerId>,
        name: &str,
        transport: McpTransport,
    ) -> Result<CommandOutput, CommandError> {
        let server = self.store_mcp_server(id, name, transport)?;
        if http_url(&server.transport).is_none() || server.needs_login {
            return Ok(CommandOutput::McpServer { server });
        }
        let id = server.id.clone();
        let asked = tokio::time::timeout(AUTH_CHECK_WAIT, self.ask_mcp_auth(id.clone())).await;
        if asked.is_err() {
            // A slow server still gets its answer, just not in this one.
            self.check_mcp_auth(id.clone());
        }
        let server = self.lock().state.mcp_server(&id).cloned().unwrap_or(server);
        Ok(CommandOutput::McpServer { server })
    }

    fn store_mcp_server(
        &self,
        id: Option<McpServerId>,
        name: &str,
        transport: McpTransport,
    ) -> Result<McpServer, CommandError> {
        let name = name.trim();
        if !mcp::valid_name(name) {
            return Err(invalid(
                "an MCP server's name needs letters, digits, - or _, and at most 64 of them",
            ));
        }
        check_transport(&transport)?;
        let mut inner = self.lock();
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
        // Logins belong to the address they were made for.
        let same_address = existing.as_ref().is_some_and(|e| {
            http_url(&e.transport).is_some() && http_url(&e.transport) == http_url(&transport)
        });
        let profiles = existing
            .as_ref()
            .map(|e| e.profiles.clone())
            .unwrap_or_default();
        let server = McpServer {
            id: id.unwrap_or_else(|| McpServerId(crate::new_id())),
            name: name.to_owned(),
            transport,
            needs_login: same_address && existing.as_ref().is_some_and(|e| e.needs_login),
            profiles: if same_address {
                profiles
            } else {
                profiles
                    .into_iter()
                    .map(|profile| McpProfile {
                        auth: McpAuth::None,
                        ..profile
                    })
                    .collect()
            },
        };
        if mcp::name_taken(&inner.state.mcp_servers, &server) {
            return Err(invalid(format!(
                "there's already an MCP server called {}",
                server.name
            )));
        }
        if existing.is_some() && !same_address {
            inner
                .store
                .remove_mcp_logins(&server.id)
                .map_err(internal)?;
        }
        inner.put_mcp_server(server.clone()).map_err(internal)?;
        inner.refresh_mcp_sessions(None);
        Ok(server)
    }

    pub(super) fn save_mcp_profile(
        &self,
        server_id: &McpServerId,
        id: Option<McpProfileId>,
        name: &str,
        values: BTreeMap<String, String>,
    ) -> Result<CommandOutput, CommandError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(invalid("a profile needs a name"));
        }
        let mut inner = self.lock();
        let server = inner
            .state
            .mcp_server(server_id)
            .cloned()
            .ok_or_else(|| not_found("MCP server", server_id))?;
        check_values(&server.transport, &values)?;
        let existing = match &id {
            Some(id) => Some(
                server
                    .profile(id)
                    .cloned()
                    .ok_or_else(|| not_found("profile", id))?,
            ),
            None => None,
        };
        if server
            .profiles
            .iter()
            .any(|p| p.name == name && Some(&p.id) != id.as_ref())
        {
            return Err(invalid(format!(
                "{} already has a profile called {name}",
                server.name
            )));
        }
        let profile = McpProfile {
            id: id.unwrap_or_else(|| McpProfileId(crate::new_id())),
            name: name.to_owned(),
            values,
            auth: match existing {
                Some(existing) => existing.auth,
                None if server.needs_login => McpAuth::NeedsLogin,
                None => McpAuth::None,
            },
        };
        let mut profiles = server.profiles.clone();
        match profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(slot) => *slot = profile.clone(),
            None => profiles.push(profile.clone()),
        }
        inner
            .put_mcp_server(McpServer { profiles, ..server })
            .map_err(internal)?;
        inner.refresh_mcp_sessions(None);
        Ok(CommandOutput::McpProfile { profile })
    }

    /// Workspaces that used it keep the server, without a profile.
    pub(super) fn remove_mcp_profile(
        &self,
        server_id: &McpServerId,
        id: &McpProfileId,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let server = inner
            .state
            .mcp_server(server_id)
            .cloned()
            .ok_or_else(|| not_found("MCP server", server_id))?;
        if server.profile(id).is_none() {
            return Err(not_found("profile", id));
        }
        inner.store.remove_mcp_login(id).map_err(internal)?;
        let profiles = server
            .profiles
            .iter()
            .filter(|p| &p.id != id)
            .cloned()
            .collect();
        inner
            .put_mcp_server(McpServer { profiles, ..server })
            .map_err(internal)?;
        let using: Vec<(WorkspaceId, BTreeMap<McpServerId, WorkspaceMcp>)> = inner
            .state
            .workspaces
            .iter()
            .filter_map(|workspace| {
                let entry = workspace.mcp.get(server_id)?;
                (entry.profile_id.as_ref() == Some(id)).then(|| {
                    let mut mcp = workspace.mcp.clone();
                    mcp.insert(
                        server_id.clone(),
                        WorkspaceMcp {
                            profile_id: None,
                            ..entry.clone()
                        },
                    );
                    (workspace.id.clone(), mcp)
                })
            })
            .collect();
        for (workspace_id, mcp) in using {
            inner
                .put_workspace_mcp(&workspace_id, mcp)
                .map_err(internal)?;
        }
        inner.refresh_mcp_sessions(None);
        Ok(CommandOutput::Done)
    }

    pub(super) fn set_workspace_mcp(
        &self,
        workspace_id: &WorkspaceId,
        servers: BTreeMap<McpServerId, WorkspaceMcp>,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let workspace = inner
            .state
            .workspaces
            .iter()
            .find(|w| &w.id == workspace_id)
            .ok_or_else(|| not_found("workspace", workspace_id))?;
        if workspace.mcp == servers {
            return Ok(CommandOutput::Done);
        }
        for (server_id, entry) in &servers {
            let server = inner
                .state
                .mcp_server(server_id)
                .ok_or_else(|| not_found("MCP server", server_id))?;
            if let Some(profile) = &entry.profile_id
                && server.profile(profile).is_none()
            {
                return Err(invalid(format!(
                    "{} has no profile with the ID {profile}",
                    server.name
                )));
            }
        }
        inner
            .put_workspace_mcp(workspace_id, servers)
            .map_err(internal)?;
        inner.refresh_mcp_sessions(None);
        Ok(CommandOutput::Done)
    }
    /// Lists the connectors of each added Claude and Codex agent, and keeps their servers in
    /// step with that.
    pub(super) async fn refresh_mcp_connectors(&self) -> Result<CommandOutput, CommandError> {
        let (claude, codex) = {
            let inner = self.lock();
            let added = |wanted: fn(&AgentLauncher) -> bool| {
                inner
                    .state
                    .providers
                    .iter()
                    .any(|provider| self.agents.get(&provider.agent).is_some_and(wanted))
            };
            (
                added(|l| matches!(l, AgentLauncher::ClaudeAdapter)),
                added(|l| matches!(l, AgentLauncher::CodexAdapter)),
            )
        };
        if !claude && !codex {
            return Err(invalid(
                "connectors come from Claude and Codex. Add one of them to this device first",
            ));
        }
        let mut listed = vec![];
        let mut problems = vec![];
        if claude {
            match self.claude_code().await {
                Ok(binary) => {
                    let cwd = self.data_dir.join("probe");
                    tokio::fs::create_dir_all(&cwd)
                        .await
                        .map_err(|error| internal(error.into()))?;
                    match list_claude_connectors(&binary, &cwd).await {
                        Ok(found) => listed.push((ConnectorKind::Claude, found)),
                        Err(problem) => problems.push(problem),
                    }
                }
                Err(error) => problems.push(format!("{error:#}")),
            }
        }
        if codex {
            let home = self.config_home.clone();
            let found = tokio::task::spawn_blocking(move || mcp::codex_apps(&home))
                .await
                .map_err(|error| internal(error.into()))?;
            match found {
                Ok(found) => listed.push((ConnectorKind::Codex, found)),
                Err(problem) => problems.push(problem),
            }
        }
        let mut added = vec![];
        let mut removed = vec![];
        let mut inner = self.lock();
        for (kind, found) in listed {
            let sync = mcp::sync_connectors(&inner.state.mcp_servers, kind, &found, || {
                McpServerId(crate::new_id())
            });
            for server in sync.put {
                inner.put_mcp_server(server).map_err(internal)?;
            }
            for id in &sync.remove {
                inner.drop_mcp_server(id).map_err(internal)?;
            }
            added.extend(sync.added);
            removed.extend(sync.removed);
        }
        inner.refresh_mcp_sessions(None);
        Ok(CommandOutput::McpConnectors {
            added,
            removed,
            problems,
        })
    }

    /// The Claude Code that lists the claude.ai connectors: the one the adapter runs.
    async fn claude_code(&self) -> anyhow::Result<std::path::PathBuf> {
        if let Some(binary) = &self.config_home.claude_code {
            return Ok(binary.clone());
        }
        let _guard = self.adapter_install.lock().await;
        crate::agents::bundled_claude_code(&self.data_dir).await
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

    pub(super) async fn login_mcp_server(
        &self,
        id: &McpServerId,
        profile_id: &McpProfileId,
    ) -> Result<CommandOutput, CommandError> {
        let (url, headers, client_id, previous, select_account) = {
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
            let profile = server
                .profile(profile_id)
                .ok_or_else(|| not_found("profile", profile_id))?;
            // Another profile's login has a client registered with the server already.
            let previous = match inner.store.mcp_login(profile_id).map_err(internal)? {
                Some(login) => Some(login),
                None => inner.store.any_mcp_login(id).map_err(internal)?,
            };
            let select_account = server
                .profiles
                .iter()
                .any(|p| &p.id != profile_id && p.auth == McpAuth::LoggedIn);
            (
                expand(url),
                expanded(headers, Some(profile)),
                oauth_client_id.clone(),
                previous,
                select_account,
            )
        };
        let state = crate::new_id();
        let redirect_uri = self.redirect_uri();
        let (address, pending) = oauth::start(
            &self.http,
            oauth::Start {
                server_url: &url,
                headers: &headers,
                client_id: client_id.as_deref(),
                previous: previous.as_ref(),
                redirect_uri: &redirect_uri,
                state: &state,
                select_account,
            },
        )
        .await
        .map_err(|error| invalid(format!("{error:#}")))?;
        let mut logins = self.lock_logins();
        logins.retain(|_, login| login.started.elapsed() < LOGIN_TIMEOUT);
        logins.insert(
            state,
            PendingLogin {
                server_id: id.clone(),
                profile_id: profile_id.clone(),
                pending,
                started: Instant::now(),
            },
        );
        Ok(CommandOutput::McpLogin { url: address })
    }

    /// Finishes a login with what the server sent back, and returns what was logged in, like
    /// `notion (work)`.
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
        let server = inner
            .state
            .mcp_server(&login.server_id)
            .ok_or_else(|| not_found("MCP server", &login.server_id))?;
        let profile = server
            .profile(&login.profile_id)
            .ok_or_else(|| not_found("profile", &login.profile_id))?;
        let name = format!("{} ({})", server.name, profile.name);
        inner
            .store
            .set_mcp_login(&login.server_id, &login.profile_id, &tokens)
            .map_err(internal)?;
        inner.set_profile_auth(&login.server_id, &login.profile_id, McpAuth::LoggedIn);
        // Agents that already gave up on the server only try again when they start.
        inner.refresh_mcp_sessions(Some(&login.profile_id));
        Ok(name)
    }

    pub fn cancel_mcp_login(&self, state: &str) {
        self.lock_logins().remove(state);
    }

    pub(super) fn logout_mcp_server(
        &self,
        id: &McpServerId,
        profile_id: &McpProfileId,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let server = inner
            .state
            .mcp_server(id)
            .ok_or_else(|| not_found("MCP server", id))?;
        if http_url(&server.transport).is_none() {
            return Err(invalid("only HTTP servers log in"));
        }
        let profile = server
            .profile(profile_id)
            .ok_or_else(|| not_found("profile", profile_id))?;
        let was_logged_in = profile.auth == McpAuth::LoggedIn;
        inner.store.remove_mcp_login(profile_id).map_err(internal)?;
        if was_logged_in {
            inner.set_profile_auth(id, profile_id, McpAuth::NeedsLogin);
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
                    transport: found.transport,
                    profiles: vec![],
                    needs_login: false,
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

    /// Asks a server in the background whether it wants a login.
    fn check_mcp_auth(self: &Arc<Self>, id: McpServerId) {
        let core = self.clone();
        tokio::spawn(async move { core.ask_mcp_auth(id).await });
    }

    /// Asks a server whether it wants a login, so the person finds out before an agent does.
    async fn ask_mcp_auth(&self, id: McpServerId) {
        let (url, headers) = {
            let inner = self.lock();
            let Some(server) = inner.state.mcp_server(&id) else {
                return;
            };
            let McpTransport::Http { url, headers, .. } = &server.transport else {
                return;
            };
            if server.needs_login {
                return;
            }
            (expand(url), expanded(headers, None))
        };
        match oauth::challenge(&self.http, &url, &headers).await {
            Ok(Some(_)) => self.lock().set_needs_login(&id, None),
            Ok(None) => {}
            Err(error) => {
                tracing::debug!(%id, "couldn't ask an MCP server about logins: {error:#}")
            }
        }
    }

    /// Where a request on the proxy goes, if the session behind `secret` got this server.
    pub fn mcp_proxy_target(&self, secret: &str, server_id: &str) -> Option<ProxyTarget> {
        let inner = self.lock();
        let live = inner
            .live
            .values()
            .find(|live| live.proxy_secret == secret)?;
        let (server_id, profile_id) = live.mcp.iter().find(|(id, _)| id.0 == server_id)?.clone();
        let server = inner.state.mcp_server(&server_id)?;
        let McpTransport::Http { url, headers, .. } = &server.transport else {
            return None;
        };
        let profile = profile_id.as_ref().and_then(|id| server.profile(id));
        Some(ProxyTarget {
            url: expand(url),
            headers: expanded(headers, profile),
            server_id,
            profile_id,
        })
    }

    /// The token to send to a server, refreshed first if it's about to expire, or if `force`
    /// says the server just refused it.
    pub async fn mcp_access_token(
        &self,
        server_id: &McpServerId,
        id: &McpProfileId,
        force: bool,
    ) -> Option<String> {
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
                if let Err(error) = inner.store.set_mcp_login(server_id, id, &refreshed) {
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
                if let Err(error) = inner.store.set_mcp_login(server_id, id, &signed_out) {
                    tracing::error!(%id, "failed to store an expired login: {error:#}");
                }
                inner.set_profile_auth(server_id, id, McpAuth::NeedsLogin);
                None
            }
            Err(oauth::RefreshError::Failed(message)) => {
                tracing::warn!(%id, "couldn't refresh an MCP login: {message}");
                // It may still work until it expires.
                (!force).then_some(login.access_token).flatten()
            }
        }
    }

    /// The server turned a request away, so it needs a login, and so does the profile the
    /// request went with.
    pub fn mcp_needs_login(&self, id: &McpServerId, profile: Option<&McpProfileId>) {
        self.lock().set_needs_login(id, profile);
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

    fn put_workspace_mcp(
        &mut self,
        id: &WorkspaceId,
        mcp: BTreeMap<McpServerId, WorkspaceMcp>,
    ) -> anyhow::Result<()> {
        self.store.set_workspace_mcp(id, &mcp)?;
        self.emit(StateChange::WorkspaceMcpChanged {
            workspace_id: id.clone(),
            mcp,
        });
        Ok(())
    }

    fn put_changed_server(&mut self, before: &McpServer, after: McpServer) {
        if before == &after {
            return;
        }
        let id = after.id.clone();
        if let Err(error) = self.put_mcp_server(after) {
            tracing::error!(%id, "failed to store an MCP login state: {error:#}");
        }
    }

    fn set_profile_auth(&mut self, id: &McpServerId, profile: &McpProfileId, auth: McpAuth) {
        let Some(server) = self.state.mcp_server(id).cloned() else {
            return;
        };
        let after = McpServer {
            profiles: server
                .profiles
                .iter()
                .map(|p| McpProfile {
                    auth: if &p.id == profile { auth } else { p.auth },
                    ..p.clone()
                })
                .collect(),
            ..server.clone()
        };
        self.put_changed_server(&server, after);
    }

    /// `profile` was turned away even with its login, if it had one.
    fn set_needs_login(&mut self, id: &McpServerId, profile: Option<&McpProfileId>) {
        let Some(server) = self.state.mcp_server(id).cloned() else {
            return;
        };
        let mut after = needing_login(server.clone());
        if let Some(profile) = profile
            && let Some(slot) = after.profiles.iter_mut().find(|p| &p.id == profile)
        {
            slot.auth = McpAuth::NeedsLogin;
        }
        self.put_changed_server(&server, after);
    }

    /// Stops the agents whose MCP servers changed, so their next prompt starts them with the
    /// new ones. ACP can't change a running session's servers. An agent that's working or
    /// waiting finishes first. `login` also counts agents that use that profile.
    pub(super) fn refresh_mcp_sessions(&mut self, login: Option<&McpProfileId>) {
        let mut outdated: Vec<SessionId> = vec![];
        for topic in self.state.topics() {
            let current = fingerprint(&self.state, topic);
            for session in &topic.sessions {
                if let Some(live) = self.live.get(&session.id)
                    && (live.mcp_fingerprint != current
                        || login.is_some_and(|id| {
                            live.mcp
                                .iter()
                                .any(|(_, profile)| profile.as_ref() == Some(id))
                        }))
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
