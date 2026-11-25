//! `tectr mcp`: the device's library of MCP servers, their profiles, and which ones each
//! workspace's agents get.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use engine::Engine;
use protocol::{
    Command, CommandOutput, DaemonState, MachineId, McpAuth, McpProfile, McpProfileId, McpServer,
    McpServerId, McpTransport, Workspace, WorkspaceId, WorkspaceMcp,
};
use tokio::sync::broadcast::error::RecvError;

use crate::{choose_machine, describe_failures, online_devices, short};

/// Long enough to sign in and approve in a browser, short enough that a forgotten login
/// doesn't hang a script forever.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

const ENV_HELP: &str = "Values can name the daemon's environment as ${NAME}, which the daemon \
                        fills in. Single-quote them so your shell leaves them alone, as in \
                        --env 'GITHUB_TOKEN=${GITHUB_TOKEN}'.";

#[derive(Subcommand)]
pub enum McpCmd {
    /// List the device's library, with profiles, and what each workspace uses.
    Ls {
        /// The machine to list, by name or ID prefix. Lists every online machine without it.
        #[arg(long)]
        on: Option<String>,
    },
    /// Add a server to the library. A stdio server is the command after `--`, an HTTP
    /// server is `--url`. No workspace gets it until it's used there.
    #[command(
        after_help = ENV_HELP,
        override_usage = "tectr mcp add NAME [--on M] [--env K=V]... -- COMMAND [ARGS]...\n       \
                          tectr mcp add NAME --url URL [--header K=V]... [--client-id ID] [--on M]"
    )]
    Add(Add),
    /// Remove a server from the library and every workspace, and forget its logins.
    Rm(Server),
    /// Give a workspace's agents a server, or change its profile or on and off there.
    Use {
        server: String,
        /// The workspace, by name or ID prefix.
        #[arg(long)]
        workspace: String,
        /// Which of the server's profiles its agents use.
        #[arg(long)]
        profile: Option<String>,
        /// Keep it in the workspace's list without giving it to agents.
        #[arg(long)]
        off: bool,
    },
    /// Take a server out of a workspace.
    Drop {
        server: String,
        /// The workspace, by name or ID prefix.
        #[arg(long)]
        workspace: String,
    },
    /// Add the servers one workspace uses that another doesn't, with their profiles.
    Copy {
        /// The workspace to copy from, by name or ID prefix.
        #[arg(long)]
        from: String,
        /// The workspace to copy to, by name or ID prefix.
        #[arg(long)]
        to: String,
    },
    /// Add or change a profile: values laid over a stdio server's env or an HTTP server's
    /// headers.
    #[command(after_help = ENV_HELP)]
    Profile(Profile),
    /// Remove a profile and forget its login. Workspaces that used it keep the server.
    RmProfile(Which),
    /// Log a profile in to an HTTP server in the browser and wait until it's done. A
    /// profile that doesn't exist yet is added first.
    Login(Which),
    /// Forget a profile's login.
    Logout(Which),
    /// Copy the servers configured for Claude Code, Codex and OpenCode on the device.
    Import {
        /// The machine to import on, by name or ID prefix. Needed only with more than one
        /// daemon.
        #[arg(long)]
        on: Option<String>,
    },
    /// Bring in the claude.ai connectors and ChatGPT apps of the device's Claude and Codex.
    /// Use one in a workspace by its name in quotes, like "claude.ai Slack".
    Connectors {
        /// The machine to refresh, by name or ID prefix. Needed only with more than one
        /// daemon.
        #[arg(long)]
        on: Option<String>,
    },
}

#[derive(Args)]
pub struct Add {
    /// What agents call it: letters, digits, `-` and `_`.
    name: String,
    /// The machine to add it on, by name or ID prefix. Needed only with more than one daemon.
    #[arg(long)]
    on: Option<String>,
    /// An environment variable for a stdio server, as KEY=VALUE. Repeatable. Single-quote
    /// values that name ${VARS}, which the daemon fills in.
    #[arg(long = "env", value_name = "K=V", value_parser = key_value, conflicts_with = "url")]
    env: Vec<(String, String)>,
    /// The address of a streamable HTTP server.
    #[arg(long)]
    url: Option<String>,
    /// A header for an HTTP server, as KEY=VALUE. Repeatable. Single-quote values that name
    /// ${VARS}, which the daemon fills in.
    #[arg(
        long = "header",
        value_name = "K=V",
        value_parser = key_value,
        requires = "url",
        conflicts_with = "command"
    )]
    headers: Vec<(String, String)>,
    /// The OAuth client ID registered for tectr, for a server that doesn't register clients
    /// by itself.
    #[arg(long, value_name = "ID", requires = "url", conflicts_with = "command")]
    client_id: Option<String>,
    /// The program a stdio server runs and its arguments.
    #[arg(last = true, value_name = "COMMAND", conflicts_with = "url")]
    command: Vec<String>,
}

/// Names one library server.
#[derive(Args)]
pub struct Server {
    /// The server's name.
    name: String,
    /// The machine, by name or ID prefix. Needed only with more than one daemon.
    #[arg(long)]
    on: Option<String>,
}

/// Names one profile of a library server.
#[derive(Args)]
pub struct Which {
    /// The server's name.
    server: String,
    /// The profile's name.
    profile: String,
    /// The machine, by name or ID prefix. Needed only with more than one daemon.
    #[arg(long)]
    on: Option<String>,
}

#[derive(Args)]
pub struct Profile {
    #[command(flatten)]
    which: Which,
    /// A value as KEY=VALUE: an environment variable for a stdio server, a header for an
    /// HTTP one. Repeatable. Replaces the profile's values.
    #[arg(long = "value", value_name = "K=V", value_parser = key_value)]
    values: Vec<(String, String)>,
}

/// Parses `KEY=VALUE`. The value is kept as typed, `${NAME}` included, and may contain `=`.
fn key_value(text: &str) -> Result<(String, String), String> {
    match text.split_once('=') {
        Some((key, value)) if !key.is_empty() => Ok((key.to_owned(), value.to_owned())),
        _ => Err(format!("expected KEY=VALUE, got {text}")),
    }
}

pub async fn run(engine: &Engine, command: McpCmd) -> Result<()> {
    match command {
        McpCmd::Ls { on } => {
            let states: Vec<DaemonState> = match on {
                Some(on) => {
                    let machine = choose_machine(engine, Some(&on))?;
                    vec![machine_state(engine, &machine)?]
                }
                None => online_devices(engine)
                    .into_iter()
                    .filter_map(|device| device.state)
                    .collect(),
            };
            if states.is_empty() {
                bail!("no daemon is reachable\n{}", describe_failures(engine));
            }
            for state in &states {
                print_servers(state);
            }
        }
        McpCmd::Add(add) => add_server(engine, add).await?,
        McpCmd::Rm(which) => {
            let machine = choose_machine(engine, which.on.as_deref())?;
            let state = machine_state(engine, &machine)?;
            let server = library_server(&state, &which.name)?;
            engine
                .request(
                    &machine,
                    Command::RemoveMcpServer {
                        id: server.id.clone(),
                    },
                )
                .await?;
        }
        McpCmd::Use {
            server,
            workspace,
            profile,
            off,
        } => {
            let (machine, state, workspace) = resolve_workspace(engine, &workspace)?;
            let server = library_server(&state, &server)?;
            let profile_id = match profile {
                Some(name) => Some(profile_named(server, &name)?.id.clone()),
                // Keeps the profile it had, or takes the only one there is.
                None => workspace
                    .mcp
                    .get(&server.id)
                    .and_then(|entry| entry.profile_id.clone())
                    .or_else(|| match server.profiles.as_slice() {
                        [only] => Some(only.id.clone()),
                        _ => None,
                    }),
            };
            let mut servers = workspace.mcp.clone();
            servers.insert(
                server.id.clone(),
                WorkspaceMcp {
                    enabled: !off,
                    profile_id,
                },
            );
            set_workspace(engine, &machine, &workspace.id, servers).await?;
        }
        McpCmd::Drop { server, workspace } => {
            let (machine, state, workspace) = resolve_workspace(engine, &workspace)?;
            let server = library_server(&state, &server)?;
            let mut servers = workspace.mcp.clone();
            if servers.remove(&server.id).is_none() {
                bail!("{} doesn't use {}", workspace.name, server.name);
            }
            set_workspace(engine, &machine, &workspace.id, servers).await?;
        }
        McpCmd::Copy { from, to } => {
            let (from_machine, _, from) = resolve_workspace(engine, &from)?;
            let (machine, _, to) = resolve_workspace(engine, &to)?;
            if from_machine != machine {
                bail!("both workspaces have to be on the same device, which owns the library");
            }
            set_workspace(engine, &machine, &to.id, copied(&from.mcp, &to.mcp)).await?;
        }
        McpCmd::Profile(Profile { which, values }) => {
            let machine = choose_machine(engine, which.on.as_deref())?;
            let state = machine_state(engine, &machine)?;
            let server = library_server(&state, &which.server)?;
            let id = profile_named(server, &which.profile)
                .ok()
                .map(|p| p.id.clone());
            save_profile(engine, &machine, server, id, &which.profile, values).await?;
        }
        McpCmd::RmProfile(which) => {
            let machine = choose_machine(engine, which.on.as_deref())?;
            let state = machine_state(engine, &machine)?;
            let server = library_server(&state, &which.server)?;
            let profile = profile_named(server, &which.profile)?;
            engine
                .request(
                    &machine,
                    Command::RemoveMcpProfile {
                        server_id: server.id.clone(),
                        id: profile.id.clone(),
                    },
                )
                .await?;
        }
        McpCmd::Login(which) => {
            let machine = choose_machine(engine, which.on.as_deref())?;
            let state = machine_state(engine, &machine)?;
            let server = library_server(&state, &which.server)?;
            let profile = match profile_named(server, &which.profile) {
                Ok(profile) => profile.clone(),
                Err(_) => save_profile(engine, &machine, server, None, &which.profile, vec![])
                    .await?
                    .context("the daemon didn't return the new profile")?,
            };
            login(engine, &machine, server, &profile).await?;
        }
        McpCmd::Logout(which) => {
            let machine = choose_machine(engine, which.on.as_deref())?;
            let state = machine_state(engine, &machine)?;
            let server = library_server(&state, &which.server)?;
            let profile = profile_named(server, &which.profile)?;
            engine
                .request(
                    &machine,
                    Command::LogoutMcpServer {
                        id: server.id.clone(),
                        profile_id: profile.id.clone(),
                    },
                )
                .await?;
        }
        McpCmd::Import { on } => {
            let machine = choose_machine(engine, on.as_deref())?;
            let output = engine.request(&machine, Command::ImportMcpServers).await?;
            if let CommandOutput::McpImport { added, skipped } = output {
                for name in added {
                    println!("added {name}");
                }
                for reason in skipped {
                    println!("skipped {reason}");
                }
            }
        }
        McpCmd::Connectors { on } => {
            let machine = choose_machine(engine, on.as_deref())?;
            let output = engine
                .request(&machine, Command::RefreshMcpConnectors)
                .await?;
            if let CommandOutput::McpConnectors {
                added,
                removed,
                problems,
            } = output
            {
                for name in added {
                    println!("added {name}");
                }
                for name in removed {
                    println!("removed {name}");
                }
                for problem in problems {
                    println!("couldn't list: {problem}");
                }
            }
        }
    }
    Ok(())
}

fn machine_state(engine: &Engine, machine: &MachineId) -> Result<DaemonState> {
    engine
        .devices()
        .into_iter()
        .filter_map(|device| device.state)
        .find(|state| &state.machine.id == machine)
        .context("that machine has no state yet")
}

fn library_server<'a>(state: &'a DaemonState, name: &str) -> Result<&'a McpServer> {
    state
        .mcp_servers
        .iter()
        .find(|server| server.name == name)
        .with_context(|| format!("{} has no server {name}", state.machine.name))
}

fn profile_named<'a>(server: &'a McpServer, name: &str) -> Result<&'a McpProfile> {
    server
        .profiles
        .iter()
        .find(|profile| profile.name == name)
        .with_context(|| format!("{} has no profile {name}", server.name))
}

/// A workspace by its name, or else by an ID prefix, with its machine's state.
fn resolve_workspace(engine: &Engine, wanted: &str) -> Result<(MachineId, DaemonState, Workspace)> {
    let states: Vec<DaemonState> = engine
        .devices()
        .into_iter()
        .filter_map(|device| device.state)
        .collect();
    if states.is_empty() {
        bail!("no daemon is reachable\n{}", describe_failures(engine));
    }
    let find = |matches: &dyn Fn(&Workspace) -> bool| -> Vec<(DaemonState, Workspace)> {
        states
            .iter()
            .flat_map(|state| {
                state
                    .workspaces
                    .iter()
                    .filter(|w| matches(w))
                    .map(|w| (state.clone(), w.clone()))
            })
            .collect()
    };
    let mut found = find(&|w| w.name == wanted);
    if found.is_empty() {
        found = find(&|w| w.id.0.starts_with(wanted));
    }
    match found.len() {
        0 => bail!("no workspace matches {wanted}"),
        1 => {
            let (state, workspace) = found.remove(0);
            Ok((state.machine.id.clone(), state, workspace))
        }
        count => bail!("{wanted} is ambiguous: {count} workspaces match. Use an ID prefix"),
    }
}

async fn set_workspace(
    engine: &Engine,
    machine: &MachineId,
    workspace_id: &WorkspaceId,
    servers: BTreeMap<McpServerId, WorkspaceMcp>,
) -> Result<()> {
    engine
        .request(
            machine,
            Command::SetWorkspaceMcp {
                workspace_id: workspace_id.clone(),
                servers,
            },
        )
        .await?;
    Ok(())
}

/// What `to` uses with what `from` uses added. What `to` already has stays as it is.
fn copied(
    from: &BTreeMap<McpServerId, WorkspaceMcp>,
    to: &BTreeMap<McpServerId, WorkspaceMcp>,
) -> BTreeMap<McpServerId, WorkspaceMcp> {
    let mut servers = from.clone();
    servers.extend(to.clone());
    servers
}

async fn save_profile(
    engine: &Engine,
    machine: &MachineId,
    server: &McpServer,
    id: Option<McpProfileId>,
    name: &str,
    values: Vec<(String, String)>,
) -> Result<Option<McpProfile>> {
    let output = engine
        .request(
            machine,
            Command::SaveMcpProfile {
                server_id: server.id.clone(),
                id,
                name: name.to_owned(),
                values: values.into_iter().collect(),
            },
        )
        .await?;
    Ok(match output {
        CommandOutput::McpProfile { profile } => {
            println!(
                "profile {} {} {}",
                short(&profile.id.0),
                server.name,
                profile.name
            );
            Some(profile)
        }
        _ => None,
    })
}

async fn add_server(engine: &Engine, add: Add) -> Result<()> {
    let transport = match (add.url, add.command.split_first()) {
        (Some(url), _) => McpTransport::Http {
            url,
            headers: add.headers.into_iter().collect(),
            oauth_client_id: add.client_id,
        },
        (None, Some((command, args))) => McpTransport::Stdio {
            command: command.clone(),
            args: args.to_vec(),
            env: add.env.into_iter().collect(),
        },
        (None, None) => bail!("give a command after -- for a stdio server, or --url"),
    };
    let machine = choose_machine(engine, add.on.as_deref())?;
    let output = engine
        .request(
            &machine,
            Command::SaveMcpServer {
                id: None,
                name: add.name,
                transport,
            },
        )
        .await?;
    if let CommandOutput::McpServer { server } = output {
        println!("mcp {} {}", short(&server.id.0), server.name);
        if server.needs_login {
            println!("it needs a login: tectr mcp login {} PROFILE", server.name);
        }
    }
    Ok(())
}

async fn login(
    engine: &Engine,
    machine: &MachineId,
    server: &McpServer,
    profile: &McpProfile,
) -> Result<()> {
    // Already logged in would look finished at once, and a new login that keeps the state
    // as it is can't be told apart from an abandoned one.
    if profile.auth == McpAuth::LoggedIn {
        bail!(
            "{} ({}) is logged in already. Log out first to log in again",
            server.name,
            profile.name
        );
    }
    // Subscribe before asking, so a fast browser can't finish in between.
    let mut updates = engine.updates();
    let output = engine
        .request(
            machine,
            Command::LoginMcpServer {
                id: server.id.clone(),
                profile_id: profile.id.clone(),
            },
        )
        .await?;
    let CommandOutput::McpLogin { url } = output else {
        bail!("the daemon didn't return a login address");
    };
    println!("{url}");
    open_in_browser(&url);
    let what = format!("{} ({})", server.name, profile.name);
    println!("waiting for the browser to finish logging in to {what}");
    let wait = async {
        loop {
            match login_state(engine, machine, &server.id, &profile.id) {
                Some(McpAuth::LoggedIn) => return Ok(()),
                Some(_) => {}
                None => bail!("{what} was removed"),
            }
            match updates.recv().await {
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => bail!("the engine stopped"),
            }
        }
    };
    tokio::time::timeout(LOGIN_TIMEOUT, wait)
        .await
        .map_err(|_| anyhow!("the login didn't finish within 5 minutes"))??;
    println!("logged in to {what}");
    Ok(())
}

/// `None` once the profile is gone. While the machine has no state, say a reconnect, it
/// counts as still waiting.
fn login_state(
    engine: &Engine,
    machine: &MachineId,
    server: &McpServerId,
    profile: &McpProfileId,
) -> Option<McpAuth> {
    match machine_state(engine, machine) {
        Ok(state) => state
            .mcp_server(server)
            .and_then(|s| s.profile(profile))
            .map(|p| p.auth),
        Err(_) => Some(McpAuth::None),
    }
}

/// Best effort: the URL is printed too, for a machine without a browser.
fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn describe_transport(transport: &McpTransport) -> String {
    let keys = |label: &str, map: &BTreeMap<String, String>| {
        if map.is_empty() {
            String::new()
        } else {
            let keys: Vec<_> = map.keys().map(String::as_str).collect();
            format!(" [{label} {}]", keys.join(", "))
        }
    };
    match transport {
        McpTransport::Stdio { command, args, env } => {
            let mut line = format!("stdio {command}");
            for arg in args {
                line.push(' ');
                line.push_str(arg);
            }
            line + &keys("env", env)
        }
        McpTransport::Http {
            url,
            headers,
            oauth_client_id,
        } => {
            let mut line = format!("http {url}{}", keys("headers", headers));
            if let Some(id) = oauth_client_id {
                line.push_str(&format!(" [client id {id}]"));
            }
            line
        }
        McpTransport::ClaudeConnector { url } => format!("claude.ai connector {url}"),
        McpTransport::CodexApp { id } => format!("ChatGPT app {id}"),
        McpTransport::Unknown => "unknown transport".to_owned(),
    }
}

fn auth_label(auth: McpAuth) -> &'static str {
    match auth {
        McpAuth::NeedsLogin => " needs login",
        McpAuth::LoggedIn => " logged in",
        McpAuth::None | McpAuth::Unknown => "",
    }
}

fn describe_server(server: &McpServer) -> String {
    let mut line = format!("{} {}", server.name, describe_transport(&server.transport));
    if server.needs_login {
        line.push_str(" needs login");
    }
    line
}

fn describe_profile(profile: &McpProfile) -> String {
    let mut line = profile.name.clone();
    if !profile.values.is_empty() {
        let keys: Vec<_> = profile.values.keys().map(String::as_str).collect();
        line.push_str(&format!(" [{}]", keys.join(", ")));
    }
    line + auth_label(profile.auth)
}

/// What a workspace's agents get, like `notion (work), files off`.
fn workspace_uses(state: &DaemonState, workspace: &Workspace) -> String {
    let names: Vec<String> = state
        .mcp_servers
        .iter()
        .filter_map(|server| {
            let entry = workspace.mcp.get(&server.id)?;
            let mut name = server.name.clone();
            if let Some(profile) = entry.profile_id.as_ref().and_then(|id| server.profile(id)) {
                name.push_str(&format!(" ({})", profile.name));
            }
            if !entry.enabled {
                name.push_str(" off");
            }
            Some(name)
        })
        .collect();
    if names.is_empty() {
        "nothing".to_owned()
    } else {
        names.join(", ")
    }
}

fn print_servers(state: &DaemonState) {
    println!("{}", state.machine.name);
    for server in &state.mcp_servers {
        println!("  server {}", describe_server(server));
        for profile in &server.profiles {
            println!("    profile {}", describe_profile(profile));
        }
    }
    for workspace in &state.workspaces {
        println!(
            "  workspace {} {} uses {}",
            short(&workspace.id.0),
            workspace.name,
            workspace_uses(state, workspace)
        );
    }
}

/// One line for `tectr status`: the library, with the profiles that need a login. Connectors
/// no workspace uses are left out, because there can be many.
pub fn status_line(state: &DaemonState) -> Option<String> {
    let used = |server: &McpServer| {
        state
            .workspaces
            .iter()
            .any(|w| w.mcp.contains_key(&server.id))
    };
    let servers: Vec<String> = state
        .mcp_servers
        .iter()
        .filter(|s| !s.transport.is_connector() || used(s))
        .map(|s| {
            let waiting: Vec<_> = s
                .profiles
                .iter()
                .filter(|p| p.auth == McpAuth::NeedsLogin)
                .map(|p| p.name.as_str())
                .collect();
            if waiting.is_empty() {
                s.name.clone()
            } else {
                format!("{} ({} needs login)", s.name, waiting.join(", "))
            }
        })
        .collect();
    (!servers.is_empty()).then(|| format!("mcp {}", servers.join(", ")))
}

/// One line for a workspace in `tectr status`, when it uses any server.
pub fn workspace_status_line(state: &DaemonState, workspace: &Workspace) -> Option<String> {
    (!workspace.mcp.is_empty()).then(|| format!("mcp {}", workspace_uses(state, workspace)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn key_value_keeps_the_value_as_typed() {
        assert_eq!(
            key_value("GITHUB_TOKEN=${GITHUB_TOKEN}"),
            Ok(("GITHUB_TOKEN".to_owned(), "${GITHUB_TOKEN}".to_owned()))
        );
        assert_eq!(
            key_value("Authorization=Bearer a=b"),
            Ok(("Authorization".to_owned(), "Bearer a=b".to_owned()))
        );
        assert_eq!(key_value("EMPTY="), Ok(("EMPTY".to_owned(), String::new())));
    }

    #[test]
    fn key_value_needs_a_key_and_an_equals_sign() {
        assert!(key_value("NOVALUE").is_err());
        assert!(key_value("=value").is_err());
        assert!(key_value("").is_err());
    }

    #[derive(Parser)]
    struct Wrapper {
        #[command(subcommand)]
        command: McpCmd,
    }

    fn parse(args: &[&str]) -> Result<McpCmd, clap::Error> {
        Wrapper::try_parse_from(std::iter::once("mcp").chain(args.iter().copied()))
            .map(|wrapper| wrapper.command)
    }

    #[test]
    fn add_parses_a_stdio_server() {
        let Ok(McpCmd::Add(add)) = parse(&[
            "add",
            "github",
            "--env",
            "TOKEN=${TOKEN}",
            "--",
            "npx",
            "-y",
            "server",
        ]) else {
            panic!("expected add");
        };
        assert_eq!(add.name, "github");
        assert_eq!(add.env, vec![("TOKEN".to_owned(), "${TOKEN}".to_owned())]);
        assert_eq!(add.command, vec!["npx", "-y", "server"]);
        assert!(add.url.is_none());
    }

    #[test]
    fn add_parses_an_http_server() {
        let Ok(McpCmd::Add(add)) = parse(&[
            "add",
            "linear",
            "--url",
            "https://mcp.linear.app/mcp",
            "--header",
            "X-Key=${KEY}",
            "--client-id",
            "abc",
        ]) else {
            panic!("expected add");
        };
        assert_eq!(add.url.as_deref(), Some("https://mcp.linear.app/mcp"));
        assert_eq!(add.headers, vec![("X-Key".to_owned(), "${KEY}".to_owned())]);
        assert_eq!(add.client_id.as_deref(), Some("abc"));
    }

    #[test]
    fn add_rejects_mixing_stdio_and_http() {
        assert!(parse(&["add", "x", "--url", "https://a", "--", "cmd"]).is_err());
        assert!(parse(&["add", "x", "--url", "https://a", "--env", "A=b"]).is_err());
        assert!(parse(&["add", "x", "--header", "A=b", "--", "cmd"]).is_err());
    }

    #[test]
    fn use_and_login_parse_their_names() {
        assert!(parse(&["use", "notion"]).is_err(), "it needs a workspace");
        let Ok(McpCmd::Use {
            server, profile, ..
        }) = parse(&["use", "notion", "--workspace", "tectr", "--profile", "work"])
        else {
            panic!("expected use");
        };
        assert_eq!(
            (server.as_str(), profile.as_deref()),
            ("notion", Some("work"))
        );
        let Ok(McpCmd::Login(which)) = parse(&["login", "notion", "work"]) else {
            panic!("expected login");
        };
        assert_eq!(
            (which.server.as_str(), which.profile.as_str()),
            ("notion", "work")
        );
        assert!(parse(&["login", "notion"]).is_err(), "it needs a profile");
    }

    fn profile(name: &str, auth: McpAuth) -> McpProfile {
        McpProfile {
            id: name.into(),
            name: name.to_owned(),
            values: BTreeMap::new(),
            auth,
        }
    }

    fn server(name: &str, profiles: Vec<McpProfile>) -> McpServer {
        McpServer {
            id: McpServerId::from(name),
            name: name.to_owned(),
            transport: McpTransport::Stdio {
                command: "run".to_owned(),
                args: vec![],
                env: BTreeMap::new(),
            },
            profiles,
            needs_login: false,
        }
    }

    fn entry(enabled: bool, profile: Option<&str>) -> WorkspaceMcp {
        WorkspaceMcp {
            enabled,
            profile_id: profile.map(McpProfileId::from),
        }
    }

    #[test]
    fn describes_transports() {
        let stdio = McpTransport::Stdio {
            command: "npx".to_owned(),
            args: vec!["-y".to_owned(), "fs".to_owned()],
            env: BTreeMap::from([("TOKEN".to_owned(), "${TOKEN}".to_owned())]),
        };
        assert_eq!(describe_transport(&stdio), "stdio npx -y fs [env TOKEN]");
        let http = McpTransport::Http {
            url: "https://a/mcp".to_owned(),
            headers: BTreeMap::from([("X-Key".to_owned(), "v".to_owned())]),
            oauth_client_id: Some("abc".to_owned()),
        };
        assert_eq!(
            describe_transport(&http),
            "http https://a/mcp [headers X-Key] [client id abc]"
        );
        let mut work = profile("work", McpAuth::NeedsLogin);
        work.values.insert("TOKEN".to_owned(), "x".to_owned());
        assert_eq!(describe_profile(&work), "work [TOKEN] needs login");
    }

    #[test]
    fn copying_adds_what_is_missing_and_keeps_what_is_there() {
        let from = BTreeMap::from([
            ("notion".into(), entry(true, Some("work"))),
            ("files".into(), entry(false, None)),
        ]);
        let to = BTreeMap::from([("notion".into(), entry(true, Some("personal")))]);
        assert_eq!(
            copied(&from, &to),
            BTreeMap::from([
                ("notion".into(), entry(true, Some("personal"))),
                ("files".into(), entry(false, None)),
            ])
        );
    }

    #[test]
    fn status_lines_show_logins_needed_and_what_workspaces_use() {
        let mut state = DaemonState {
            machine: protocol::Machine {
                id: "m".into(),
                name: "mac".into(),
            },
            workspaces: vec![Workspace {
                id: "w".into(),
                name: "repo".into(),
                path: "/repo".into(),
                topics: vec![],
                mcp: BTreeMap::new(),
            }],
            approvals: vec![],
            questions: vec![],
            session_options: Default::default(),
            session_tasks: Default::default(),
            providers: vec![],
            drafts: BTreeMap::new(),
            draft_images: BTreeMap::new(),
            commands: BTreeMap::new(),
            settings: protocol::DeviceSettings::default(),
            awake_until_ms: None,
            mcp_servers: vec![],
            paired_clients: vec![],
            pairing_requests: vec![],
            clones: vec![],
        };
        state.mcp_servers = vec![
            server(
                "notion",
                vec![
                    profile("work", McpAuth::LoggedIn),
                    profile("personal", McpAuth::NeedsLogin),
                ],
            ),
            server("files", vec![]),
        ];
        assert_eq!(
            status_line(&state).as_deref(),
            Some("mcp notion (personal needs login), files")
        );
        assert_eq!(workspace_status_line(&state, &state.workspaces[0]), None);
        state.workspaces[0].mcp = BTreeMap::from([
            ("notion".into(), entry(true, Some("work"))),
            ("files".into(), entry(false, None)),
        ]);
        assert_eq!(
            workspace_status_line(&state, &state.workspaces[0]).as_deref(),
            Some("mcp notion (work), files off")
        );
    }
}
