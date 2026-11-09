//! What MCP servers an agent gets, and keeping it from loading any others.
//!
//! tectr owns the MCP servers: each agent's own configuration is switched off, and the
//! servers of the session's topic are passed over ACP instead. HTTP servers go through the
//! daemon's proxy, which adds the login, so agents never see a token.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use protocol::{DaemonState, McpServer, McpServerId, McpTransport, Topic};
use serde_json::{Map, Value, json};

use crate::agents::AgentLauncher;

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

/// The servers a topic's agents get. `proxy` is the address the daemon proxies a session's
/// HTTP servers at, which the server's ID is added to.
pub fn agent_servers(
    state: &DaemonState,
    topic: &Topic,
    proxy: &str,
) -> (Vec<McpServerId>, Vec<AgentMcp>) {
    let mut ids = vec![];
    let mut servers = vec![];
    for server in state.topic_mcp_servers(topic) {
        let agent = match &server.transport {
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
    (ids, servers)
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

/// Where each agent keeps its configuration. Tests point these at a temporary folder.
#[derive(Debug, Clone)]
pub struct ConfigHome {
    pub home: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub xdg_config: Option<PathBuf>,
    pub opencode_config: Option<PathBuf>,
    pub opencode_content: Option<String>,
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
        }
    }

    fn codex_config(&self) -> Option<PathBuf> {
        self.codex_home
            .clone()
            .or_else(|| self.home.as_ref().map(|home| home.join(".codex")))
            .map(|dir| dir.join("config.toml"))
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

/// How each agent's own servers are switched off. Codex and OpenCode can only switch them off
/// one by one, so their config files are read for the names each time an agent starts. `keep`
/// are tectr's own names: an agent's server with one of those is replaced rather than off.
pub fn isolation(
    launcher: &AgentLauncher,
    cwd: &Path,
    keep: &[&str],
    home: &ConfigHome,
) -> Isolation {
    match launcher {
        // Strict mode leaves out config files, plugins and agent definitions, and the variable
        // stops Claude Code fetching the claude.ai connectors.
        AgentLauncher::ClaudeAdapter => Isolation {
            args: vec![],
            env: vec![("ENABLE_CLAUDEAI_MCP_SERVERS".to_owned(), "false".to_owned())],
            meta: Some(
                json!({ "claudeCode": { "options": { "strictMcpConfig": true } } })
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            ),
        },
        // `-c mcp_servers={}` merges rather than replaces, so it does nothing. Apps are
        // ChatGPT's connectors, and plugins bring servers of their own.
        AgentLauncher::CodexAdapter => {
            let mut args = vec![
                "-c".to_owned(),
                "features.apps=false".to_owned(),
                "-c".to_owned(),
                "features.plugins=false".to_owned(),
            ];
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

/// Why a server can't be saved under this name, if it can't. A device-wide name has to be
/// unique on the device, and a topic's own name among the device's and that topic's.
pub fn name_taken(servers: &[McpServer], candidate: &McpServer) -> bool {
    servers.iter().any(|other| {
        other.id != candidate.id
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
    fn claude_gets_strict_mode_and_no_connectors() {
        let dir = tempfile::tempdir().unwrap();
        let isolation = isolation(
            &AgentLauncher::ClaudeAdapter,
            dir.path(),
            &[],
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
            &["github"],
            &home(dir.path()),
        );
        assert_eq!(
            isolation.args,
            [
                "-c",
                "features.apps=false",
                "-c",
                "features.plugins=false",
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
        let isolation = isolation(&AgentLauncher::OpenCode, &nested, &["project"], &config);
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
            isolation(&AgentLauncher::OpenCode, dir.path(), &[], &home(dir.path())),
            Isolation::default()
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
