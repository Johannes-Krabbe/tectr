//! Which agents this daemon can start, and how.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use protocol::PermissionMode;
use tokio::process::Command;

/// Pinned so that an upstream release can't break a running setup. Change it
/// deliberately, after checking the new adapter against the daemon.
const CLAUDE_ADAPTER_VERSION: &str = "0.100.0";
const CLAUDE_ADAPTER_PACKAGE: &str = "@agentclientprotocol/claude-agent-acp";
const MIN_NODE_MAJOR: u32 = 22;

#[derive(Debug, Clone)]
pub enum AgentLauncher {
    /// The Claude adapter, installed on demand into the daemon's data directory.
    ClaudeAdapter,
    /// Any other command that speaks ACP on stdio.
    Command { program: PathBuf, args: Vec<String> },
}

pub fn default_agents() -> BTreeMap<String, AgentLauncher> {
    BTreeMap::from([("claude".to_owned(), AgentLauncher::ClaudeAdapter)])
}

impl AgentLauncher {
    /// Returns the program and arguments to run, installing the adapter first
    /// if needed. The caller must not run two installs at the same time.
    pub async fn resolve(&self, data_dir: &Path) -> Result<(PathBuf, Vec<String>)> {
        match self {
            Self::Command { program, args } => Ok((program.clone(), args.clone())),
            Self::ClaudeAdapter => {
                check_node().await?;
                let entry = install_claude_adapter(data_dir).await?;
                Ok((PathBuf::from("node"), vec![entry.display().to_string()]))
            }
        }
    }
}

/// tectr's permission modes, as the mode IDs of the Claude adapter. Other
/// agents will need their own mapping when they're added.
pub fn acp_mode_id(mode: PermissionMode) -> Option<&'static str> {
    match mode {
        PermissionMode::Manual => Some("default"),
        PermissionMode::Auto => Some("auto"),
        PermissionMode::Yolo => Some("bypassPermissions"),
        PermissionMode::Unknown => None,
    }
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

async fn install_claude_adapter(data_dir: &Path) -> Result<PathBuf> {
    let versions = data_dir.join("adapters").join("claude-agent-acp");
    let install_dir = versions.join(CLAUDE_ADAPTER_VERSION);
    let entry = adapter_entry(&install_dir);
    if tokio::fs::try_exists(&entry).await? {
        return Ok(entry);
    }

    // Install into a temporary directory, so an interrupted install never
    // looks like a finished one.
    let partial = versions.join(format!("{CLAUDE_ADAPTER_VERSION}.partial"));
    if tokio::fs::try_exists(&partial).await? {
        tokio::fs::remove_dir_all(&partial).await?;
    }
    tokio::fs::create_dir_all(&partial).await?;

    tracing::info!(
        version = CLAUDE_ADAPTER_VERSION,
        "installing the Claude adapter"
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
        .arg(format!("{CLAUDE_ADAPTER_PACKAGE}@{CLAUDE_ADAPTER_VERSION}"))
        .output()
        .await
        .context("failed to run npm, which installs the Claude adapter")?;
    if !output.status.success() {
        bail!(
            "failed to install the Claude adapter: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    tokio::fs::rename(&partial, &install_dir).await?;
    Ok(entry)
}

fn adapter_entry(install_dir: &Path) -> PathBuf {
    install_dir
        .join("node_modules")
        .join(CLAUDE_ADAPTER_PACKAGE)
        .join("dist/index.js")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_mode_has_an_agent_mode() {
        for mode in [
            PermissionMode::Manual,
            PermissionMode::Auto,
            PermissionMode::Yolo,
        ] {
            assert!(acp_mode_id(mode).is_some());
        }
        assert_eq!(acp_mode_id(PermissionMode::Unknown), None);
    }
}
