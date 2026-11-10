//! What MCP servers an agent gets, and keeping it from loading any others.
//!
//! tectr owns the MCP servers: each agent's own configuration is switched off, and the
//! servers of the session's topic are passed over ACP instead. HTTP servers go through the
//! daemon's proxy, which adds the login, so agents never see a token. The exception is an
//! agent's connectors, whose logins its company holds: the ones a topic turns on are let
//! through by name or address, and the agent connects to them by itself.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use protocol::{DaemonState, McpServer, McpServerId, McpTransport, Topic};
use serde_json::{Map, Value, json};

use crate::agents::AgentLauncher;

/// A connector a topic turns on, which only its own agent can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentConnector {
    Claude { url: String },
    Codex { id: String },
}

/// A server as one agent process gets it, with `${NAME}` values filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentMcp {
    Stdio {
        name: String,
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    /// Always the daemon's proxy, never the server itself.
    Http { name: String, url: String },
}

impl AgentMcp {
    pub fn name(&self) -> &str {
        match self {
            Self::Stdio { name, .. } | Self::Http { name, .. } => name,
        }
    }
}

/// What a topic's agents get from tectr.
#[derive(Debug, Default)]
pub struct TopicMcp {
    /// The servers passed over ACP, and the IDs of the ones among them the proxy serves.
    pub ids: Vec<McpServerId>,
    pub servers: Vec<AgentMcp>,
    pub connectors: Vec<AgentConnector>,
}

/// The servers a topic's agents get. `proxy` is the address the daemon proxies a session's
/// HTTP servers at, which the server's ID is added to.
pub fn agent_servers(state: &DaemonState, topic: &Topic, proxy: &str) -> TopicMcp {
    let mut ids = vec![];
    let mut servers = vec![];
    let mut connectors = vec![];
    for server in state.topic_mcp_servers(topic) {
        let agent = match &server.transport {
            McpTransport::ClaudeConnector { url } => {
                connectors.push(AgentConnector::Claude { url: url.clone() });
                continue;
            }
            McpTransport::CodexApp { id } => {
                connectors.push(AgentConnector::Codex { id: id.clone() });
                continue;
            }
            McpTransport::Stdio { command, args, env } => AgentMcp::Stdio {
                name: server.name.clone(),
                command: expand(command),
                args: args.iter().map(|arg| expand(arg)).collect(),
                env: env
                    .iter()
                    .map(|(key, value)| (key.clone(), expand(value)))
                    .collect(),
            },
            McpTransport::Http { .. } => AgentMcp::Http {
                name: server.name.clone(),
                url: format!("{proxy}/{}", server.id),
            },
            McpTransport::Unknown => continue,
        };
        ids.push(server.id.clone());
        servers.push(agent);
    }
    TopicMcp {
        ids,
        servers,
        connectors,
    }
}

/// Fills in `${NAME}` from the daemon's environment, which is the login shell's when it runs
/// with `--shell-env`. A name that isn't set becomes empty, the way a shell does it.
pub fn expand(value: &str) -> String {
    expand_with(value, |name| std::env::var(name).ok())
}

fn expand_with(value: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) if is_env_name(&after[..end]) => {
                let name = &after[..end];
                match lookup(name) {
                    Some(found) => out.push_str(&found),
                    None => tracing::warn!(name, "an MCP server names an unset variable"),
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_env_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Agents use the name in tool names and config keys, so it stays to what all of them accept.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What keeps an agent from loading MCP servers of its own, on top of how it's started.
#[derive(Debug, Default, PartialEq)]
pub struct Isolation {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Goes into the `_meta` of `session/new` and `session/resume`.
    pub meta: Option<Map<String, Value>>,
}

/// Where each agent keeps its configuration and conversations. Tests point these at a
/// temporary folder.
#[derive(Debug, Clone)]
pub struct ConfigHome {
    pub home: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub xdg_config: Option<PathBuf>,
    pub opencode_config: Option<PathBuf>,
    pub opencode_content: Option<String>,
    /// The Claude Code the adapter runs instead of the one it ships with, which is what
    /// lists the claude.ai connectors too.
    pub claude_code: Option<PathBuf>,
}

impl ConfigHome {
    pub fn from_env() -> Self {
        let var = |name| std::env::var_os(name).filter(|value| !value.is_empty());
        Self {
            home: dirs::home_dir(),
            codex_home: var("CODEX_HOME").map(PathBuf::from),
            xdg_config: var("XDG_CONFIG_HOME").map(PathBuf::from),
            opencode_config: var("OPENCODE_CONFIG").map(PathBuf::from),
            opencode_content: std::env::var("OPENCODE_CONFIG_CONTENT").ok(),
            claude_code: var("CLAUDE_CODE_EXECUTABLE").map(PathBuf::from),
        }
    }

    pub fn codex_dir(&self) -> Option<PathBuf> {
        self.codex_home
            .clone()
            .or_else(|| self.home.as_ref().map(|home| home.join(".codex")))
    }

    pub fn claude_dir(&self) -> Option<PathBuf> {
        self.home.as_ref().map(|home| home.join(".claude"))
    }

    fn codex_config(&self) -> Option<PathBuf> {
        self.codex_dir().map(|dir| dir.join("config.toml"))
    }

    fn opencode_dir(&self) -> Option<PathBuf> {
        self.xdg_config
            .clone()
            .or_else(|| self.home.as_ref().map(|home| home.join(".config")))
            .map(|dir| dir.join("opencode"))
    }

    /// OpenCode's global files, in the order it reads them.
    fn opencode_global(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = self
            .opencode_dir()
            .map(|dir| {
                ["config.json", "opencode.json", "opencode.jsonc"]
                    .into_iter()
                    .map(|name| dir.join(name))
                    .collect()
            })
            .unwrap_or_default();
        files.extend(self.opencode_config.clone());
        files
    }
}

fn claude_meta(options: Value) -> Option<Map<String, Value>> {
    json!({ "claudeCode": { "options": options } })
        .as_object()
        .cloned()
}

/// How each agent's own servers are switched off, apart from the `connectors` its topic turns
/// on. Codex and OpenCode can only switch servers off one by one, so their config files are
/// read for the names each time an agent starts. An agent's server with the name of one of
/// tectr's `servers` is replaced rather than off.
pub fn isolation(
    launcher: &AgentLauncher,
    cwd: &Path,
    servers: &[AgentMcp],
    connectors: &[AgentConnector],
    home: &ConfigHome,
) -> Isolation {
    let keep: Vec<&str> = servers.iter().map(AgentMcp::name).collect();
    match launcher {
        AgentLauncher::ClaudeAdapter => {
            let urls: Vec<&str> = connectors
                .iter()
                .filter_map(|connector| match connector {
                    AgentConnector::Claude { url } => Some(url.as_str()),
                    AgentConnector::Codex { .. } => None,
                })
                .collect();
            if urls.is_empty() {
                // Strict mode leaves out config files, plugins and agent definitions, and the
                // variable stops Claude Code fetching the claude.ai connectors.
                return Isolation {
                    args: vec![],
                    env: vec![("ENABLE_CLAUDEAI_MCP_SERVERS".to_owned(), "false".to_owned())],
                    meta: claude_meta(json!({ "strictMcpConfig": true })),
                };
            }
            // Strict mode would leave out the connectors too, so an allowlist lets through
            // only tectr's servers and the chosen connectors. Claude Code matches a stdio
            // server by name, and once the list has an address, every remote one by address.
            // A connector's address is its service's own, not Anthropic's proxy.
            let mut allowed: Vec<Value> = servers
                .iter()
                .map(|server| match server {
                    AgentMcp::Stdio { name, .. } => json!({ "serverName": name }),
                    AgentMcp::Http { url, .. } => json!({ "serverUrl": url }),
                })
                .collect();
            allowed.extend(urls.iter().map(|url| json!({ "serverUrl": url })));
            Isolation {
                args: vec![],
                // A daemon started from inside a tectr agent would pass on its `false`.
                env: vec![("ENABLE_CLAUDEAI_MCP_SERVERS".to_owned(), "true".to_owned())],
                meta: claude_meta(json!({ "settings": { "allowedMcpServers": allowed } })),
            }
        }
        // `-c mcp_servers={}` merges rather than replaces, so it does nothing. Plugins bring
        // servers of their own.
        AgentLauncher::CodexAdapter => {
            let ids: Vec<&str> = connectors
                .iter()
                .filter_map(|connector| match connector {
                    // Codex splits the path on dots, so an ID it couldn't take apart stays off.
                    AgentConnector::Codex { id } if valid_name(id) => Some(id.as_str()),
                    AgentConnector::Codex { id } => {
                        tracing::warn!(id, "can't turn on a ChatGPT app with this ID");
                        None
                    }
                    AgentConnector::Claude { .. } => None,
                })
                .collect();
            let mut args = vec!["-c".to_owned(), "features.plugins=false".to_owned()];
            if ids.is_empty() {
                args.extend(["-c".to_owned(), "features.apps=false".to_owned()]);
            } else {
                // Apps are ChatGPT's connectors. Off by default, so an app connected since the
                // last refresh stays off until it's turned on here.
                args.extend([
                    "-c".to_owned(),
                    "features.apps=true".to_owned(),
                    "-c".to_owned(),
                    "apps._default.enabled=false".to_owned(),
                ]);
                for id in ids {
                    args.extend(["-c".to_owned(), format!("apps.{id}.enabled=true")]);
                }
            }
            let files = [
                home.codex_config(),
                Some(cwd.join(".codex").join("config.toml")),
            ];
            let mut names = BTreeSet::new();
            for file in files.into_iter().flatten() {
                names.extend(codex_names(&file));
            }
            for name in names.iter().filter(|name| !keep.contains(&name.as_str())) {
                // Codex splits the path on dots, so a name it couldn't take apart stays on.
                if valid_name(name) {
                    args.push("-c".to_owned());
                    args.push(format!("mcp_servers.{name}.enabled=false"));
                } else {
                    tracing::warn!(name, "can't switch off a Codex MCP server with this name");
                }
            }
            Isolation {
                args,
                env: vec![],
                meta: None,
            }
        }
        AgentLauncher::OpenCode => {
            let mut files = home.opencode_global();
            files.extend(opencode_project_files(cwd));
            let mut names = BTreeSet::new();
            for file in &files {
                names.extend(opencode_names(file));
            }
            // Whatever the person already passes this way stays, with the switches merged in.
            let mut content = home
                .opencode_content
                .as_deref()
                .and_then(|text| serde_json::from_str::<Value>(&strip_jsonc(text)).ok())
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            if let Some(Value::Object(mcp)) = content.get("mcp") {
                names.extend(mcp.keys().cloned());
            }
            let off: Map<String, Value> = names
                .iter()
                .filter(|name| !keep.contains(&name.as_str()))
                .map(|name| (name.clone(), json!({ "enabled": false })))
                .collect();
            if off.is_empty() && home.opencode_content.is_none() {
                return Isolation::default();
            }
            let mcp = content
                .entry("mcp")
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(mcp) = mcp {
                for (name, value) in off {
                    match mcp.get_mut(&name) {
                        Some(Value::Object(existing)) => {
                            existing.insert("enabled".to_owned(), Value::Bool(false));
                        }
                        _ => {
                            mcp.insert(name, value);
                        }
                    }
                }
            }
            Isolation {
                args: vec![],
                env: vec![(
                    "OPENCODE_CONFIG_CONTENT".to_owned(),
                    Value::Object(content).to_string(),
                )],
                meta: None,
            }
        }
        AgentLauncher::Command { .. } => Isolation::default(),
    }
}

/// OpenCode reads `opencode.json` and `.opencode/` in every folder from the project up to its
/// git root.
fn opencode_project_files(cwd: &Path) -> Vec<PathBuf> {
    let mut files = vec![];
    for dir in cwd.ancestors() {
        for name in ["opencode.json", "opencode.jsonc"] {
            files.push(dir.join(name));
            files.push(dir.join(".opencode").join(name));
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    files
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn codex_table(path: &Path) -> Option<toml_edit::Table> {
    let text = read(path)?;
    match text.parse::<toml_edit::DocumentMut>() {
        Ok(document) => document
            .get("mcp_servers")
            .and_then(|item| item.as_table())
            .cloned(),
        Err(error) => {
            tracing::warn!(path = %path.display(), "can't read a Codex config: {error}");
            None
        }
    }
}

fn codex_names(path: &Path) -> Vec<String> {
    codex_table(path)
        .map(|table| table.iter().map(|(name, _)| name.to_owned()).collect())
        .unwrap_or_default()
}

fn json_file(path: &Path) -> Option<Map<String, Value>> {
    let text = read(path)?;
    match serde_json::from_str::<Value>(&strip_jsonc(&text)) {
        Ok(Value::Object(object)) => Some(object),
        Ok(_) => None,
        Err(error) => {
            tracing::warn!(path = %path.display(), "can't read a config file: {error}");
            None
        }
    }
}

fn opencode_names(path: &Path) -> Vec<String> {
    match json_file(path).and_then(|config| config.get("mcp").cloned()) {
        Some(Value::Object(mcp)) => mcp.keys().cloned().collect(),
        _ => vec![],
    }
}

/// Takes comments and trailing commas out of JSONC, so plain JSON parsing can read it.
pub fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut previous = ' ';
                for skipped in chars.by_ref() {
                    if previous == '*' && skipped == '/' {
                        break;
                    }
                    previous = skipped;
                }
            }
            _ => out.push(c),
        }
    }
    remove_trailing_commas(&out)
}

fn remove_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let chars: Vec<char> = text.chars().collect();
    for (index, &c) in chars.iter().enumerate() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
        }
        if c == ',' {
            let next = chars[index + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}' | ']')) {
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// A server found in another agent's configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub transport: McpTransport,
    pub enabled: bool,
}

/// Every server configured for Claude Code, Codex and OpenCode on this device, with why each
/// one that can't come along can't. A name seen twice keeps the first.
pub fn import(home: &ConfigHome) -> (Vec<Found>, Vec<String>) {
    let mut found = vec![];
    let mut skipped = vec![];
    if let Some(home_dir) = &home.home {
        claude_servers(&home_dir.join(".claude.json"), &mut found, &mut skipped);
    }
    if let Some(file) = home.codex_config() {
        codex_servers(&file, &mut found, &mut skipped);
    }
    for file in home.opencode_global() {
        opencode_servers(&file, &mut found, &mut skipped);
    }
    let mut seen = BTreeSet::new();
    found.retain(|server| seen.insert(server.name.clone()));
    (found, skipped)
}

fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        _ => vec![],
    }
}

fn string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    match value {
        Some(Value::Object(map)) => map
            .iter()
            .filter_map(|(key, value)| value.as_str().map(|v| (key.clone(), v.to_owned())))
            .collect(),
        _ => BTreeMap::new(),
    }
}

fn claude_servers(path: &Path, found: &mut Vec<Found>, skipped: &mut Vec<String>) {
    let Some(Value::Object(servers)) = json_file(path).and_then(|c| c.get("mcpServers").cloned())
    else {
        return;
    };
    for (name, server) in servers {
        let kind = server
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("stdio");
        let transport = match kind {
            "stdio" => server
                .get("command")
                .and_then(Value::as_str)
                .map(|command| McpTransport::Stdio {
                    command: command.to_owned(),
                    args: strings(server.get("args")),
                    env: string_map(server.get("env")),
                }),
            "http" => server
                .get("url")
                .and_then(Value::as_str)
                .map(|url| McpTransport::Http {
                    url: url.to_owned(),
                    headers: string_map(server.get("headers")),
                    oauth_client_id: None,
                }),
            _ => None,
        };
        push(found, skipped, "Claude Code", name, transport, kind, true);
    }
}

fn codex_servers(path: &Path, found: &mut Vec<Found>, skipped: &mut Vec<String>) {
    let Some(table) = codex_table(path) else {
        return;
    };
    for (name, item) in table.iter() {
        let Some(server) = item.as_table_like() else {
            continue;
        };
        let text = |key| server.get(key).and_then(|i| i.as_str()).map(str::to_owned);
        let map = |key| -> BTreeMap<String, String> {
            server
                .get(key)
                .and_then(|i| i.as_table_like())
                .map(|t| {
                    t.iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.to_owned(), v.to_owned())))
                        .collect()
                })
                .unwrap_or_default()
        };
        let enabled = server
            .get("enabled")
            .and_then(|i| i.as_bool())
            .unwrap_or(true);
        let transport = if let Some(command) = text("command") {
            let args = server
                .get("args")
                .and_then(|i| i.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            Some(McpTransport::Stdio {
                command,
                args,
                env: map("env"),
            })
        } else {
            text("url").map(|url| {
                let mut headers = map("http_headers");
                for (header, variable) in map("env_http_headers") {
                    headers.insert(header, format!("${{{variable}}}"));
                }
                if let Some(variable) = text("bearer_token_env_var") {
                    headers.insert(
                        "Authorization".to_owned(),
                        format!("Bearer ${{{variable}}}"),
                    );
                }
                McpTransport::Http {
                    url,
                    headers,
                    oauth_client_id: None,
                }
            })
        };
        push(
            found,
            skipped,
            "Codex",
            name.to_owned(),
            transport,
            "unknown",
            enabled,
        );
    }
}

fn opencode_servers(path: &Path, found: &mut Vec<Found>, skipped: &mut Vec<String>) {
    let Some(Value::Object(servers)) = json_file(path).and_then(|c| c.get("mcp").cloned()) else {
        return;
    };
    for (name, server) in servers {
        let kind = server
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("local");
        let enabled = server
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let transport = match kind {
            "local" => {
                let mut command = strings(server.get("command")).into_iter();
                command.next().map(|program| McpTransport::Stdio {
                    command: program,
                    args: command.collect(),
                    env: string_map(server.get("environment")),
                })
            }
            "remote" => server
                .get("url")
                .and_then(Value::as_str)
                .map(|url| McpTransport::Http {
                    url: url.to_owned(),
                    headers: string_map(server.get("headers")),
                    oauth_client_id: None,
                }),
            _ => None,
        };
        push(found, skipped, "OpenCode", name, transport, kind, enabled);
    }
}

fn push(
    found: &mut Vec<Found>,
    skipped: &mut Vec<String>,
    source: &str,
    name: String,
    transport: Option<McpTransport>,
    kind: &str,
    enabled: bool,
) {
    if !valid_name(&name) {
        skipped.push(format!(
            "{name} ({source}): names can only have letters, digits, - and _"
        ));
        return;
    }
    match transport {
        Some(transport) => found.push(Found {
            name,
            transport,
            enabled,
        }),
        None if kind == "sse" => skipped.push(format!(
            "{name} ({source}): SSE servers aren't supported, only stdio and streamable HTTP"
        )),
        None => skipped.push(format!(
            "{name} ({source}): its configuration isn't complete"
        )),
    }
}

/// A connector as its agent listed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub transport: McpTransport,
}

/// The claude.ai connectors in what `claude mcp list` prints, one per line as
/// `claude.ai Slack: https://mcp.slack.com/mcp - Connected`. The name is Claude Code's own.
pub fn claude_connectors(output: &str) -> Vec<Listed> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("claude.ai ")?;
            let (name, rest) = rest.split_once(": ")?;
            let url = rest.rsplit_once(" - ").map_or(rest, |(url, _)| url).trim();
            (url.starts_with("https://") || url.starts_with("http://")).then(|| Listed {
                name: format!("claude.ai {name}"),
                transport: McpTransport::ClaudeConnector {
                    url: url.to_owned(),
                },
            })
        })
        .collect()
}

/// The ChatGPT apps Codex last listed, from the tools it keeps in its cache. OpenAI's own
/// built-in apps, like Pets, aren't connectors anyone connected, so they're left out.
pub fn codex_apps(home: &ConfigHome) -> Result<Vec<Listed>, String> {
    let missing = || {
        "Codex has no list of its apps on this device yet. Using an app in Codex once saves one"
            .to_owned()
    };
    let dir = home
        .codex_dir()
        .ok_or_else(missing)?
        .join("cache")
        .join("codex_apps_tools");
    let entries = std::fs::read_dir(&dir).map_err(|_| missing())?;
    // One file per account. The newest is the one Codex uses now.
    let newest = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
        .ok_or_else(missing)?;
    let cache = json_file(&newest)
        .ok_or_else(|| format!("can't read Codex's list of apps in {}", newest.display()))?;
    let mut seen = BTreeSet::new();
    let mut apps = vec![];
    for tool in cache
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let text = |key| tool.get(key).and_then(Value::as_str);
        let (Some(id), Some(name)) = (text("connector_id"), text("connector_name")) else {
            continue;
        };
        if id.starts_with("connector_openai_") || !seen.insert(id.to_owned()) {
            continue;
        }
        apps.push(Listed {
            name: format!("ChatGPT {name}"),
            transport: McpTransport::CodexApp { id: id.to_owned() },
        });
    }
    Ok(apps)
}

/// Which agent a connector belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorKind {
    Claude,
    Codex,
}

impl ConnectorKind {
    pub fn of(transport: &McpTransport) -> Option<Self> {
        match transport {
            McpTransport::ClaudeConnector { .. } => Some(Self::Claude),
            McpTransport::CodexApp { .. } => Some(Self::Codex),
            _ => None,
        }
    }
}

/// How the connector servers of one agent change to match what it listed.
#[derive(Debug, Default, PartialEq)]
pub struct ConnectorSync {
    /// New ones, and ones whose name changed.
    pub put: Vec<McpServer>,
    pub remove: Vec<McpServerId>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// A new connector starts off, because each one brings many tools. One that's still there
/// keeps its own on or off and every topic's.
pub fn sync_connectors(
    existing: &[McpServer],
    kind: ConnectorKind,
    listed: &[Listed],
    mut new_id: impl FnMut() -> McpServerId,
) -> ConnectorSync {
    let mut sync = ConnectorSync::default();
    let ours: Vec<&McpServer> = existing
        .iter()
        .filter(|server| ConnectorKind::of(&server.transport) == Some(kind))
        .collect();
    for found in listed {
        match ours
            .iter()
            .find(|server| server.transport == found.transport)
        {
            Some(server) if server.name == found.name => {}
            Some(server) => sync.put.push(McpServer {
                name: found.name.clone(),
                ..(*server).clone()
            }),
            None => {
                sync.added.push(found.name.clone());
                sync.put.push(McpServer {
                    id: new_id(),
                    name: found.name.clone(),
                    topic_id: None,
                    transport: found.transport.clone(),
                    enabled: false,
                    auth: protocol::McpAuth::None,
                });
            }
        }
    }
    for server in ours {
        if !listed
            .iter()
            .any(|found| found.transport == server.transport)
        {
            sync.removed.push(server.name.clone());
            sync.remove.push(server.id.clone());
        }
    }
    sync
}

/// Why a server can't be saved under this name, if it can't. A device-wide name has to be
/// unique on the device, and a topic's own name among the device's and that topic's.
/// Connectors have names no other server can, so they're left out.
pub fn name_taken(servers: &[McpServer], candidate: &McpServer) -> bool {
    servers.iter().any(|other| {
        other.id != candidate.id
            && !other.transport.is_connector()
            && other.name == candidate.name
            && (other.topic_id.is_none()
                || candidate.topic_id.is_none()
                || other.topic_id == candidate.topic_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(dir: &Path) -> ConfigHome {
        ConfigHome {
            home: Some(dir.to_owned()),
            codex_home: None,
            xdg_config: None,
            opencode_config: None,
            opencode_content: None,
            claude_code: None,
        }
    }

    fn stdio_server(name: &str) -> AgentMcp {
        AgentMcp::Stdio {
            name: name.into(),
            command: "x".into(),
            args: vec![],
            env: vec![],
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn variables_are_filled_in_and_anything_else_is_left_alone() {
        let lookup = |name: &str| (name == "TOKEN").then(|| "secret".to_owned());
        assert_eq!(expand_with("Bearer ${TOKEN}", lookup), "Bearer secret");
        assert_eq!(expand_with("${MISSING}x", lookup), "x");
        assert_eq!(
            expand_with("${not a name} $TOKEN ${", lookup),
            "${not a name} $TOKEN ${"
        );
        assert_eq!(expand_with("${TOKEN}${TOKEN}", lookup), "secretsecret");
    }

    #[test]
    fn connectors_a_topic_turns_on_go_to_isolation_rather_than_over_acp() {
        let state: DaemonState = serde_json::from_value(json!({
            "machine": { "id": "m", "name": "m" },
            "workspaces": [],
            "approvals": [],
            "mcp_servers": [
                { "id": "files", "name": "files", "enabled": true,
                  "transport": { "type": "stdio", "command": "x" } },
                { "id": "slack", "name": "claude.ai Slack", "enabled": false,
                  "transport": { "type": "claude_connector", "url": "https://mcp.slack.com/mcp" } },
                { "id": "drive", "name": "ChatGPT Google Drive", "enabled": true,
                  "transport": { "type": "codex_app", "id": "connector_2" } },
                { "id": "notion", "name": "claude.ai Notion", "enabled": false,
                  "transport": { "type": "claude_connector", "url": "https://mcp.notion.com/mcp" } },
            ],
        }))
        .unwrap();
        let topic: Topic = serde_json::from_value(json!({
            "id": "t", "name": "t", "worktree_id": "w", "worktrees": [], "sessions": [],
            "mcp": { "slack": true },
        }))
        .unwrap();
        let got = agent_servers(&state, &topic, "http://proxy");
        assert_eq!(got.ids, [McpServerId::from("files")]);
        assert_eq!(got.servers.len(), 1);
        assert_eq!(
            got.connectors,
            [
                AgentConnector::Claude {
                    url: "https://mcp.slack.com/mcp".into()
                },
                AgentConnector::Codex {
                    id: "connector_2".into()
                },
            ]
        );
    }

    #[test]
    fn claude_without_connectors_gets_strict_mode_and_none_of_them() {
        let dir = tempfile::tempdir().unwrap();
        // A ChatGPT app is nothing to Claude.
        let codex = [AgentConnector::Codex { id: "app_1".into() }];
        let isolation = isolation(
            &AgentLauncher::ClaudeAdapter,
            dir.path(),
            &[stdio_server("files")],
            &codex,
            &home(dir.path()),
        );
        assert_eq!(
            isolation.env,
            [("ENABLE_CLAUDEAI_MCP_SERVERS".to_owned(), "false".to_owned())]
        );
        assert_eq!(
            Value::Object(isolation.meta.unwrap()),
            json!({ "claudeCode": { "options": { "strictMcpConfig": true } } })
        );
    }

    #[test]
    fn claude_with_connectors_lets_through_only_tectrs_servers_and_those() {
        let dir = tempfile::tempdir().unwrap();
        let servers = [
            stdio_server("files"),
            AgentMcp::Http {
                name: "linear".into(),
                url: "http://127.0.0.1:7769/mcp/secret/x1".into(),
            },
        ];
        let connectors = [
            AgentConnector::Claude {
                url: "https://mcp.slack.com/mcp".into(),
            },
            AgentConnector::Codex { id: "app_1".into() },
        ];
        let isolation = isolation(
            &AgentLauncher::ClaudeAdapter,
            dir.path(),
            &servers,
            &connectors,
            &home(dir.path()),
        );
        assert_eq!(
            isolation.env,
            [("ENABLE_CLAUDEAI_MCP_SERVERS".to_owned(), "true".to_owned())]
        );
        assert_eq!(
            Value::Object(isolation.meta.unwrap()),
            json!({ "claudeCode": { "options": { "settings": { "allowedMcpServers": [
                { "serverName": "files" },
                { "serverUrl": "http://127.0.0.1:7769/mcp/secret/x1" },
                { "serverUrl": "https://mcp.slack.com/mcp" },
            ] } } } })
        );
    }

    #[test]
    fn codex_turns_on_only_the_chosen_apps() {
        let dir = tempfile::tempdir().unwrap();
        let connectors = [
            AgentConnector::Codex {
                id: "asdk_app_1".into(),
            },
            AgentConnector::Codex {
                id: "has.dot".into(),
            },
            AgentConnector::Claude {
                url: "https://mcp.slack.com/mcp".into(),
            },
        ];
        let isolation = isolation(
            &AgentLauncher::CodexAdapter,
            dir.path(),
            &[],
            &connectors,
            &home(dir.path()),
        );
        assert_eq!(
            isolation.args,
            [
                "-c",
                "features.plugins=false",
                "-c",
                "features.apps=true",
                "-c",
                "apps._default.enabled=false",
                "-c",
                "apps.asdk_app_1.enabled=true",
            ]
        );
    }

    #[test]
    fn codex_switches_off_each_of_its_servers_except_tectrs_own() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".codex/config.toml"),
            "model = \"x\"\n[mcp_servers.linear]\nurl = \"https://l\"\n[mcp_servers.github]\ncommand = \"gh\"\n[mcp_servers.\"has.dot\"]\ncommand = \"x\"\n",
        );
        let project = dir.path().join("repo");
        write(
            &project.join(".codex/config.toml"),
            "[mcp_servers.local]\ncommand = \"y\"\n",
        );
        let isolation = isolation(
            &AgentLauncher::CodexAdapter,
            &project,
            &[stdio_server("github")],
            &[],
            &home(dir.path()),
        );
        assert_eq!(
            isolation.args,
            [
                "-c",
                "features.plugins=false",
                "-c",
                "features.apps=false",
                "-c",
                "mcp_servers.linear.enabled=false",
                "-c",
                "mcp_servers.local.enabled=false",
            ]
        );
    }

    #[test]
    fn opencode_switches_off_global_and_project_servers_and_keeps_what_was_passed() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".config/opencode/opencode.jsonc"),
            "{\n  // a comment\n  \"mcp\": { \"global\": { \"type\": \"local\", \"command\": [\"x\"], }, },\n}",
        );
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let nested = repo.join("sub");
        write(
            &repo.join("opencode.json"),
            r#"{"mcp":{"project":{"type":"remote","url":"u"}}}"#,
        );
        std::fs::create_dir_all(&nested).unwrap();
        let mut config = home(dir.path());
        config.opencode_content =
            Some(r#"{"theme":"x","mcp":{"passed":{"type":"local","command":["y"]}}}"#.to_owned());
        let isolation = isolation(
            &AgentLauncher::OpenCode,
            &nested,
            &[stdio_server("project")],
            &[],
            &config,
        );
        let (name, value) = &isolation.env[0];
        assert_eq!(name, "OPENCODE_CONFIG_CONTENT");
        let value: Value = serde_json::from_str(value).unwrap();
        assert_eq!(
            value,
            json!({
                "theme": "x",
                "mcp": {
                    "passed": { "type": "local", "command": ["y"], "enabled": false },
                    "global": { "enabled": false },
                }
            })
        );
    }

    #[test]
    fn opencode_with_nothing_configured_starts_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            isolation(
                &AgentLauncher::OpenCode,
                dir.path(),
                &[],
                &[],
                &home(dir.path())
            ),
            Isolation::default()
        );
    }

    #[test]
    fn claude_connectors_are_read_from_mcp_list() {
        let output = "Checking MCP server health...\n\n\
            github: npx -y gh-mcp - \u{2714} Connected\n\
            claude.ai Slack: https://mcp.slack.com/mcp - \u{2714} Connected\n\
            claude.ai Google Drive: https://drivemcp.googleapis.com/mcp/v1 - ! Needs authentication\n\
            claude.ai Broken: not a url - x Failed\n";
        assert_eq!(
            claude_connectors(output),
            [
                Listed {
                    name: "claude.ai Slack".into(),
                    transport: McpTransport::ClaudeConnector {
                        url: "https://mcp.slack.com/mcp".into()
                    },
                },
                Listed {
                    name: "claude.ai Google Drive".into(),
                    transport: McpTransport::ClaudeConnector {
                        url: "https://drivemcp.googleapis.com/mcp/v1".into()
                    },
                },
            ]
        );
    }

    #[test]
    fn codex_apps_come_from_its_newest_cache_without_openais_own() {
        let dir = tempfile::tempdir().unwrap();
        let config = home(dir.path());
        let error = codex_apps(&config).unwrap_err();
        assert!(error.contains("no list"), "{error}");

        let cache = dir.path().join(".codex/cache/codex_apps_tools");
        write(
            &cache.join("old.json"),
            r#"{"tools":[{"connector_id":"gone","connector_name":"Gone"}]}"#,
        );
        // The newer file has to look newer to the file system.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(
            &cache.join("new.json"),
            r#"{"schema_version":1,"tools":[
                {"connector_id":"asdk_app_1","connector_name":"Slack","tool_name":"a"},
                {"connector_id":"asdk_app_1","connector_name":"Slack","tool_name":"b"},
                {"connector_id":"connector_openai_pets","connector_name":"Pets"},
                {"connector_id":"connector_2","connector_name":"Google Drive"}
            ]}"#,
        );
        let names: Vec<_> = codex_apps(&config)
            .unwrap()
            .into_iter()
            .map(|app| (app.name, app.transport))
            .collect();
        assert_eq!(
            names,
            [
                (
                    "ChatGPT Slack".to_owned(),
                    McpTransport::CodexApp {
                        id: "asdk_app_1".into()
                    }
                ),
                (
                    "ChatGPT Google Drive".to_owned(),
                    McpTransport::CodexApp {
                        id: "connector_2".into()
                    }
                ),
            ]
        );
    }

    #[test]
    fn syncing_adds_new_connectors_off_renames_and_removes_only_that_agents() {
        let server = |id: &str, name: &str, transport: McpTransport, enabled| McpServer {
            id: id.into(),
            name: name.into(),
            topic_id: None,
            transport,
            enabled,
            auth: protocol::McpAuth::None,
        };
        let claude = |url: &str| McpTransport::ClaudeConnector { url: url.into() };
        let existing = [
            server("a", "claude.ai Slack", claude("https://slack"), true),
            server("b", "claude.ai Old", claude("https://old"), true),
            server("c", "claude.ai Linear", claude("https://linear"), true),
            server(
                "d",
                "ChatGPT Slack",
                McpTransport::CodexApp { id: "s".into() },
                false,
            ),
            server("e", "files", McpTransport::Unknown, true),
        ];
        let listed = [
            Listed {
                name: "claude.ai Slack".into(),
                transport: claude("https://slack"),
            },
            Listed {
                name: "claude.ai Linear (2)".into(),
                transport: claude("https://linear"),
            },
            Listed {
                name: "claude.ai Notion".into(),
                transport: claude("https://notion"),
            },
        ];
        let sync = sync_connectors(&existing, ConnectorKind::Claude, &listed, || "new".into());
        assert_eq!(
            sync,
            ConnectorSync {
                put: vec![
                    server("c", "claude.ai Linear (2)", claude("https://linear"), true),
                    server("new", "claude.ai Notion", claude("https://notion"), false),
                ],
                remove: vec!["b".into()],
                added: vec!["claude.ai Notion".into()],
                removed: vec!["claude.ai Old".into()],
            }
        );
    }

    #[test]
    fn import_reads_all_three_agents_and_says_what_it_left_out() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".claude.json"),
            r#"{"mcpServers":{
                "github":{"command":"gh-mcp","args":["--x"],"env":{"T":"1"}},
                "linear":{"type":"http","url":"https://mcp.linear.app/mcp"},
                "old":{"type":"sse","url":"https://old"},
                "bad name":{"command":"x"}
            }}"#,
        );
        write(
            &dir.path().join(".codex/config.toml"),
            "[mcp_servers.github]\ncommand = \"other\"\n[mcp_servers.sentry]\nurl = \"https://s\"\nbearer_token_env_var = \"SENTRY\"\nenabled = false\n",
        );
        write(
            &dir.path().join(".config/opencode/opencode.json"),
            r#"{"mcp":{"local":{"type":"local","command":["run","it"],"environment":{"A":"b"}}}}"#,
        );
        let (found, skipped) = import(&home(dir.path()));
        let names: Vec<_> = found.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["github", "linear", "sentry", "local"]);
        assert_eq!(
            found[0].transport,
            McpTransport::Stdio {
                command: "gh-mcp".into(),
                args: vec!["--x".into()],
                env: BTreeMap::from([("T".into(), "1".into())]),
            }
        );
        assert_eq!(
            found[2],
            Found {
                name: "sentry".into(),
                transport: McpTransport::Http {
                    url: "https://s".into(),
                    headers: BTreeMap::from([("Authorization".into(), "Bearer ${SENTRY}".into())]),
                    oauth_client_id: None,
                },
                enabled: false,
            }
        );
        assert_eq!(
            found[3].transport,
            McpTransport::Stdio {
                command: "run".into(),
                args: vec!["it".into()],
                env: BTreeMap::from([("A".into(), "b".into())]),
            }
        );
        assert_eq!(skipped.len(), 2, "{skipped:?}");
        assert!(
            skipped
                .iter()
                .any(|s| s.starts_with("old (Claude Code): SSE"))
        );
        assert!(
            skipped
                .iter()
                .any(|s| s.starts_with("bad name (Claude Code)"))
        );
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_go_but_strings_stay() {
        let text = "{ \"a\": \"// not a comment, }\", /* gone */ \"b\": [1, 2,], }";
        let value: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(value, json!({ "a": "// not a comment, }", "b": [1, 2] }));
    }

    #[test]
    fn a_topic_can_reuse_a_name_only_another_topic_has() {
        let server = |id: &str, topic: Option<&str>| McpServer {
            id: id.into(),
            name: "db".into(),
            topic_id: topic.map(Into::into),
            transport: McpTransport::Unknown,
            enabled: true,
            auth: protocol::McpAuth::None,
        };
        let existing = [server("a", Some("t1"))];
        assert!(!name_taken(&existing, &server("b", Some("t2"))));
        assert!(name_taken(&existing, &server("b", Some("t1"))));
        assert!(name_taken(&existing, &server("b", None)));
        assert!(
            !name_taken(&existing, &server("a", Some("t1"))),
            "a server keeps its own name"
        );
    }
}
