//! Which agents this daemon can start, and how.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use protocol::PermissionMode;
use tokio::process::Command;

/// Pinned so that an upstream release can't break a running setup. Change it
/// deliberately, after checking the new adapter against the daemon.
const CLAUDE_ADAPTER: NpmAdapter = NpmAdapter {
    label: "Claude",
    directory: "claude-agent-acp",
    package: "@agentclientprotocol/claude-agent-acp",
    version: "0.100.0",
    entry: "node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
};
const CODEX_ADAPTER: NpmAdapter = NpmAdapter {
    label: "Codex",
    directory: "codex-acp",
    package: "@zed-industries/codex-acp",
    version: "0.16.0",
    entry: "node_modules/.bin/codex-acp",
};
const MIN_NODE_MAJOR: u32 = 22;

/// An ACP adapter published to npm, installed on demand into the data directory.
struct NpmAdapter {
    label: &'static str,
    directory: &'static str,
    package: &'static str,
    version: &'static str,
    /// What to run, relative to the install directory.
    entry: &'static str,
}

#[derive(Debug, Clone)]
pub enum AgentLauncher {
    /// The Claude adapter, installed on demand into the daemon's data directory.
    ClaudeAdapter,
    /// The Codex adapter, installed the same way. It uses the machine's Codex login.
    CodexAdapter,
    /// OpenCode speaks ACP itself, through `opencode acp`.
    OpenCode,
    /// Any other command that speaks ACP on stdio.
    Command { program: PathBuf, args: Vec<String> },
}

pub fn default_agents() -> BTreeMap<String, AgentLauncher> {
    BTreeMap::from([
        ("claude".to_owned(), AgentLauncher::ClaudeAdapter),
        ("codex".to_owned(), AgentLauncher::CodexAdapter),
        ("opencode".to_owned(), AgentLauncher::OpenCode),
    ])
}

impl AgentLauncher {
    /// Returns the program and arguments to run, installing the adapter first
    /// if needed. The caller must not run two installs at the same time.
    pub async fn resolve(&self, data_dir: &Path) -> Result<(PathBuf, Vec<String>)> {
        match self {
            Self::Command { program, args } => Ok((program.clone(), args.clone())),
            Self::OpenCode => Ok((PathBuf::from("opencode"), vec!["acp".to_owned()])),
            Self::CodexAdapter => Ok((install_adapter(data_dir, &CODEX_ADAPTER).await?, vec![])),
            Self::ClaudeAdapter => {
                check_node().await?;
                let entry = install_adapter(data_dir, &CLAUDE_ADAPTER).await?;
                Ok((PathBuf::from("node"), vec![entry.display().to_string()]))
            }
        }
    }

    /// What people call this agent. `agent` is the name it was configured under.
    pub fn display_name(&self, agent: &str) -> String {
        match self {
            Self::ClaudeAdapter => "Claude Code".to_owned(),
            Self::CodexAdapter => "Codex".to_owned(),
            Self::OpenCode => "OpenCode".to_owned(),
            Self::Command { .. } => agent.to_owned(),
        }
    }

    /// Whether asking this agent what it offers is harmless enough to do unprompted.
    /// Each time OpenCode is asked, it adds an empty session to the person's history.
    pub fn refreshes_on_start(&self) -> bool {
        !matches!(self, Self::OpenCode)
    }

    /// Why this agent can't run on this machine, if it can't. It starts nothing.
    pub async fn missing(&self) -> Option<String> {
        let not_installed = |program, name| {
            (!on_path(program)).then(|| format!("{name} isn't installed on this device"))
        };
        match self {
            Self::Command { .. } => None,
            Self::OpenCode => not_installed("opencode", "OpenCode"),
            Self::CodexAdapter => {
                not_installed("codex", "Codex").or_else(|| not_installed("npm", "npm"))
            }
            Self::ClaudeAdapter => match not_installed("claude", "Claude Code") {
                Some(reason) => Some(reason),
                None => check_node().await.err().map(|error| format!("{error:#}")),
            },
        }
    }

    /// tectr's permission modes, as this agent's own mode IDs. `None` means the
    /// agent has nothing that honors that mode.
    pub fn mode_id(&self, mode: PermissionMode) -> Option<&'static str> {
        match (self, mode) {
            (_, PermissionMode::Unknown) => None,
            // Approval is required to edit files, which is what manual promises.
            (Self::CodexAdapter, PermissionMode::Manual) => Some("read-only"),
            (Self::CodexAdapter, PermissionMode::Auto) => Some("auto"),
            (Self::CodexAdapter, PermissionMode::Yolo) => Some("full-access"),
            // OpenCode's modes are build and plan, with no permission levels. In
            // build, its own config decides what needs asking, which is auto.
            (Self::OpenCode, PermissionMode::Auto) => Some("build"),
            (Self::OpenCode, _) => None,
            (_, PermissionMode::Manual) => Some("default"),
            (_, PermissionMode::Auto) => Some("auto"),
            (_, PermissionMode::Yolo) => Some("bypassPermissions"),
        }
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// The Claude adapter runs on Node, so a daemon that uses it needs Node too.
pub async fn check_node() -> Result<()> {
    let output = Command::new("node").arg("--version").output().await.context(
        "Node.js wasn't found on PATH. The Claude adapter needs Node 22 or newer on this machine",
    )?;
    let version = String::from_utf8_lossy(&output.stdout);
    let major = version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok());
    match major {
        Some(major) if major >= MIN_NODE_MAJOR => Ok(()),
        _ => bail!(
            "the Claude adapter needs Node {MIN_NODE_MAJOR} or newer, found {}",
            version.trim()
        ),
    }
}

async fn install_adapter(data_dir: &Path, adapter: &NpmAdapter) -> Result<PathBuf> {
    let versions = data_dir.join("adapters").join(adapter.directory);
    let install_dir = versions.join(adapter.version);
    let entry = install_dir.join(adapter.entry);
    if tokio::fs::try_exists(&entry).await? {
        return Ok(entry);
    }

    // Install into a temporary directory, so an interrupted install never
    // looks like a finished one.
    let partial = versions.join(format!("{}.partial", adapter.version));
    if tokio::fs::try_exists(&partial).await? {
        tokio::fs::remove_dir_all(&partial).await?;
    }
    tokio::fs::create_dir_all(&partial).await?;

    tracing::info!(
        adapter = adapter.label,
        version = adapter.version,
        "installing an agent adapter"
    );
    let output = Command::new("npm")
        .arg("install")
        .arg("--prefix")
        .arg(&partial)
        .args([
            "--no-audit",
            "--no-fund",
            "--ignore-scripts",
            "--save-exact",
        ])
        .arg(format!("{}@{}", adapter.package, adapter.version))
        .output()
        .await
        .with_context(|| {
            format!(
                "failed to run npm, which installs the {} adapter",
                adapter.label
            )
        })?;
    if !output.status.success() {
        bail!(
            "failed to install the {} adapter: {}",
            adapter.label,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    tokio::fs::rename(&partial, &install_dir).await?;
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_map_to_what_each_agent_offers() {
        let custom = AgentLauncher::Command {
            program: "agent".into(),
            args: vec![],
        };
        for agent in [
            &AgentLauncher::ClaudeAdapter,
            &AgentLauncher::CodexAdapter,
            &custom,
        ] {
            for mode in [
                PermissionMode::Manual,
                PermissionMode::Auto,
                PermissionMode::Yolo,
            ] {
                assert!(agent.mode_id(mode).is_some());
            }
            assert_eq!(agent.mode_id(PermissionMode::Unknown), None);
        }
        // OpenCode has no permission levels, so it must not claim manual or yolo.
        assert_eq!(
            AgentLauncher::OpenCode.mode_id(PermissionMode::Auto),
            Some("build")
        );
        assert_eq!(
            AgentLauncher::OpenCode.mode_id(PermissionMode::Manual),
            None
        );
        assert_eq!(AgentLauncher::OpenCode.mode_id(PermissionMode::Yolo), None);
    }
}
