//! `tectr mcp`: the MCP servers a daemon gives its agents.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use engine::Engine;
use protocol::{
    Command, CommandOutput, DaemonState, MachineId, McpAuth, McpServer, McpServerId, McpTransport,
    Topic, TopicId,
};
use tokio::sync::broadcast::error::RecvError;

use crate::{choose_machine, describe_failures, online_devices, resolve_topic, short};

/// Long enough to sign in and approve in a browser, short enough that a forgotten login
/// doesn't hang a script forever.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

const ENV_HELP: &str = "Values can name the daemon's environment as ${NAME}, which the daemon \
                        fills in. Single-quote them so your shell leaves them alone, as in \
                        --env 'GITHUB_TOKEN=${GITHUB_TOKEN}'.";

#[derive(Subcommand)]
pub enum McpCmd {
    /// List the device's servers and which ones each topic gets.
    Ls {
        /// The machine to list, by name or ID prefix. Lists every online machine without it.
        #[arg(long)]
        on: Option<String>,
    },
    /// Add a server. A stdio server is the command after `--`, an HTTP server is `--url`.
    #[command(
        after_help = ENV_HELP,
        override_usage = "tectr mcp add NAME [--topic T] [--off] [--env K=V]... -- COMMAND [ARGS]...\n       \
                          tectr mcp add NAME --url URL [--header K=V]... [--client-id ID] [--topic T] [--off]"
    )]
    Add(Add),
    /// Remove a server and forget its login.
    Rm(Which),
    /// Turn a server on. Without --topic, every topic gets it unless it turns it off. With
    /// --topic, that topic gets it.
    On(Which),
    /// Turn a server off. Without --topic, topics get it only if they turn it on. With
    /// --topic, that topic doesn't get it.
    Off(Which),
    /// Make a topic follow a device server's own on or off again.
    Default {
        name: String,
        /// The topic, by name or ID prefix.
        #[arg(long)]
        topic: String,
    },
    /// Log in to an HTTP server in the browser and wait until it's done.
    Login(Which),
    /// Forget an HTTP server's login.
    Logout(Which),
    /// Copy the servers configured for Claude Code, Codex and OpenCode on the device.
    Import {
        /// The machine to import on, by name or ID prefix. Needed only with more than one
        /// daemon.
        #[arg(long)]
        on: Option<String>,
    },
    /// Bring in the claude.ai connectors and ChatGPT apps of the device's Claude and Codex.
    /// New ones start off. Turn one on by its name in quotes, like "claude.ai Slack".
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
    /// Give it to this topic only, by name or ID prefix. Without it, every topic on the
    /// device gets it.
    #[arg(long)]
    topic: Option<String>,
    /// The machine to add it on, by name or ID prefix. Needed only with more than one daemon.
    #[arg(long, conflicts_with = "topic")]
    on: Option<String>,
    /// Add it switched off.
    #[arg(long)]
    off: bool,
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

/// Names one server. A name can be taken once device-wide and once by each topic.
#[derive(Args)]
pub struct Which {
    /// The server's name.
    name: String,
    /// The topic, by name or ID prefix.
    #[arg(long)]
    topic: Option<String>,
    /// The machine, by name or ID prefix. Needed only with more than one daemon.
    #[arg(long, conflicts_with = "topic")]
    on: Option<String>,
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
            let target = Target::new(engine, &which)?;
            let server = target.own_server(&which.name)?;
            engine
                .request(
                    &target.machine,
                    Command::RemoveMcpServer {
                        id: server.id.clone(),
                    },
                )
                .await?;
        }
        McpCmd::On(which) => switch(engine, &which, true).await?,
        McpCmd::Off(which) => switch(engine, &which, false).await?,
        McpCmd::Default { name, topic } => {
            let (machine, topic_id) = resolve_topic(engine, &topic)?;
            let state = machine_state(engine, &machine)?;
            let server = device_server(&state, &name)
                .with_context(|| format!("the device has no server {name}"))?;
            engine
                .request(
                    &machine,
                    Command::SetTopicMcp {
                        topic_id,
                        server_id: server.id.clone(),
                        on: None,
                    },
                )
                .await?;
        }
        McpCmd::Login(which) => {
            let target = Target::new(engine, &which)?;
            let server = target.own_server(&which.name)?;
            login(engine, &target.machine, server).await?;
        }
        McpCmd::Logout(which) => {
            let target = Target::new(engine, &which)?;
            let server = target.own_server(&which.name)?;
            engine
                .request(
                    &target.machine,
                    Command::LogoutMcpServer {
                        id: server.id.clone(),
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

/// The machine a command goes to, its state, and the topic it's about if any.
struct Target {
    machine: MachineId,
    state: DaemonState,
    topic: Option<TopicId>,
}

impl Target {
    fn new(engine: &Engine, which: &Which) -> Result<Self> {
        let (machine, topic) = match &which.topic {
            Some(topic) => {
                let (machine, topic) = resolve_topic(engine, topic)?;
                (machine, Some(topic))
            }
            None => (choose_machine(engine, which.on.as_deref())?, None),
        };
        let state = machine_state(engine, &machine)?;
        Ok(Self {
            machine,
            state,
            topic,
        })
    }

    /// The topic's own server with `--topic`, the device's otherwise.
    fn own_server(&self, name: &str) -> Result<&McpServer> {
        match &self.topic {
            Some(topic) => topic_server(&self.state, topic, name).with_context(|| {
                let hint = if device_server(&self.state, name).is_some() {
                    ". The device has one: leave out --topic"
                } else {
                    ""
                };
                format!(
                    "topic {} has no server {name} of its own{hint}",
                    short(&topic.0)
                )
            }),
            None => device_server(&self.state, name).with_context(|| {
                let hint = if self.state.mcp_servers.iter().any(|s| s.name == name) {
                    ". A topic has one: add --topic"
                } else {
                    ""
                };
                format!("the device has no server {name}{hint}")
            }),
        }
    }
}

fn machine_state(engine: &Engine, machine: &MachineId) -> Result<DaemonState> {
    engine
        .devices()
        .into_iter()
        .filter_map(|device| device.state)
        .find(|state| &state.machine.id == machine)
        .context("that machine has no state yet")
}

fn device_server<'a>(state: &'a DaemonState, name: &str) -> Option<&'a McpServer> {
    state
        .mcp_servers
        .iter()
        .find(|server| server.topic_id.is_none() && server.name == name)
}

fn topic_server<'a>(state: &'a DaemonState, topic: &TopicId, name: &str) -> Option<&'a McpServer> {
    state
        .mcp_servers
        .iter()
        .find(|server| server.topic_id.as_ref() == Some(topic) && server.name == name)
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
    let (machine, topic_id) = match &add.topic {
        Some(topic) => {
            let (machine, topic) = resolve_topic(engine, topic)?;
            (machine, Some(topic))
        }
        None => (choose_machine(engine, add.on.as_deref())?, None),
    };
    let output = engine
        .request(
            &machine,
            Command::SaveMcpServer {
                id: None,
                name: add.name,
                topic_id,
                transport,
                enabled: !add.off,
            },
        )
        .await?;
    if let CommandOutput::McpServer { server } = output {
        println!("mcp {} {}", short(&server.id.0), server.name);
    }
    Ok(())
}

/// Resaves a server with `enabled` changed, keeping everything else as it is.
fn resave(server: &McpServer, enabled: bool) -> Command {
    Command::SaveMcpServer {
        id: Some(server.id.clone()),
        name: server.name.clone(),
        topic_id: server.topic_id.clone(),
        transport: server.transport.clone(),
        enabled,
    }
}

async fn switch(engine: &Engine, which: &Which, on: bool) -> Result<()> {
    let target = Target::new(engine, which)?;
    let command = match &target.topic {
        None => resave(target.own_server(&which.name)?, on),
        Some(topic) => match topic_server(&target.state, topic, &which.name) {
            Some(server) => resave(server, on),
            None => {
                let server = device_server(&target.state, &which.name).with_context(|| {
                    format!(
                        "neither topic {} nor the device has a server {}",
                        short(&topic.0),
                        which.name
                    )
                })?;
                Command::SetTopicMcp {
                    topic_id: topic.clone(),
                    server_id: server.id.clone(),
                    on: Some(on),
                }
            }
        },
    };
    engine.request(&target.machine, command).await?;
    Ok(())
}

async fn login(engine: &Engine, machine: &MachineId, server: &McpServer) -> Result<()> {
    // Already logged in would look finished at once, and a new login that keeps the state
    // as it is can't be told apart from an abandoned one.
    if server.auth == McpAuth::LoggedIn {
        bail!(
            "{} is logged in already. Log out first to log in again",
            server.name
        );
    }
    // Subscribe before asking, so a fast browser can't finish in between.
    let mut updates = engine.updates();
    let output = engine
        .request(
            machine,
            Command::LoginMcpServer {
                id: server.id.clone(),
            },
        )
        .await?;
    let CommandOutput::McpLogin { url } = output else {
        bail!("the daemon didn't return a login address");
    };
    println!("{url}");
    open_in_browser(&url);
    println!(
        "waiting for the browser to finish logging in to {}",
        server.name
    );
    let wait = async {
        loop {
            match login_state(engine, machine, &server.id) {
                Some(McpAuth::LoggedIn) => return Ok(()),
                Some(_) => {}
                None => bail!("{} was removed", server.name),
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
    println!("logged in to {}", server.name);
    Ok(())
}

/// `None` once the server is gone. While the machine has no state, say a reconnect, it
/// counts as still waiting.
fn login_state(engine: &Engine, machine: &MachineId, id: &McpServerId) -> Option<McpAuth> {
    match machine_state(engine, machine) {
        Ok(state) => state.mcp_server(id).map(|server| server.auth),
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

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn auth_label(auth: McpAuth) -> &'static str {
    match auth {
        McpAuth::NeedsLogin => " needs login",
        McpAuth::LoggedIn => " logged in",
        McpAuth::None | McpAuth::Unknown => "",
    }
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

fn describe_server(server: &McpServer) -> String {
    format!(
        "{} {} {}{}",
        server.name,
        describe_transport(&server.transport),
        on_off(server.enabled),
        auth_label(server.auth)
    )
}

fn topic_gets(state: &DaemonState, topic: &Topic) -> String {
    let names: Vec<_> = state
        .topic_mcp_servers(topic)
        .map(|server| server.name.as_str())
        .collect();
    if names.is_empty() {
        "nothing".to_owned()
    } else {
        names.join(", ")
    }
}

fn print_servers(state: &DaemonState) {
    println!("{}", state.machine.name);
    for server in state.mcp_servers.iter().filter(|s| s.topic_id.is_none()) {
        println!("  device {}", describe_server(server));
    }
    for workspace in &state.workspaces {
        for topic in &workspace.topics {
            println!(
                "  topic {} {}/{} gets {}",
                short(&topic.id.0),
                workspace.name,
                topic.name,
                topic_gets(state, topic)
            );
            let own = state
                .mcp_servers
                .iter()
                .filter(|s| s.topic_id.as_ref() == Some(&topic.id));
            for server in own {
                println!("    own {}", describe_server(server));
            }
            for (id, on) in &topic.mcp {
                if let Some(server) = state.mcp_server(id) {
                    println!("    override {} {}", server.name, on_off(*on));
                }
            }
        }
    }
}

/// One line for `tectr status`: the device's servers, with what needs attention. Connectors
/// that are off are left out, because there can be many.
pub fn status_line(state: &DaemonState) -> Option<String> {
    let servers: Vec<_> = state
        .mcp_servers
        .iter()
        .filter(|s| s.topic_id.is_none() && (s.enabled || !s.transport.is_connector()))
        .map(|s| format!("{} {}{}", s.name, on_off(s.enabled), auth_label(s.auth)))
        .collect();
    (!servers.is_empty()).then(|| format!("mcp {}", servers.join(", ")))
}

/// One line for a topic in `tectr status`, only when it differs from the device's defaults.
pub fn topic_status_line(state: &DaemonState, topic: &Topic) -> Option<String> {
    let has_own = state
        .mcp_servers
        .iter()
        .any(|s| s.topic_id.as_ref() == Some(&topic.id));
    (has_own || !topic.mcp.is_empty()).then(|| format!("mcp {}", topic_gets(state, topic)))
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
            "--off",
            "--",
            "npx",
            "-y",
            "server",
        ]) else {
            panic!("expected add");
        };
        assert_eq!(add.name, "github");
        assert!(add.off);
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
            "--topic",
            "feature",
        ]) else {
            panic!("expected add");
        };
        assert_eq!(add.url.as_deref(), Some("https://mcp.linear.app/mcp"));
        assert_eq!(add.headers, vec![("X-Key".to_owned(), "${KEY}".to_owned())]);
        assert_eq!(add.client_id.as_deref(), Some("abc"));
        assert_eq!(add.topic.as_deref(), Some("feature"));
    }

    #[test]
    fn add_rejects_mixing_stdio_and_http() {
        assert!(parse(&["add", "x", "--url", "https://a", "--", "cmd"]).is_err());
        assert!(parse(&["add", "x", "--url", "https://a", "--env", "A=b"]).is_err());
        assert!(parse(&["add", "x", "--header", "A=b", "--", "cmd"]).is_err());
        assert!(parse(&["add", "x", "--topic", "t", "--on", "m", "--", "cmd"]).is_err());
    }

    #[test]
    fn default_needs_a_topic() {
        assert!(parse(&["default", "github"]).is_err());
        assert!(parse(&["default", "github", "--topic", "t"]).is_ok());
    }

    fn server(name: &str, topic: Option<&str>, enabled: bool, auth: McpAuth) -> McpServer {
        McpServer {
            id: McpServerId::from(name),
            name: name.to_owned(),
            topic_id: topic.map(TopicId::from),
            transport: McpTransport::Stdio {
                command: "run".to_owned(),
                args: vec![],
                env: BTreeMap::new(),
            },
            enabled,
            auth,
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
        assert_eq!(
            describe_server(&server("fs", None, false, McpAuth::NeedsLogin)),
            "fs stdio run off needs login"
        );
    }

    fn state(servers: Vec<McpServer>, topics: Vec<Topic>) -> DaemonState {
        DaemonState {
            machine: protocol::Machine {
                id: "m".into(),
                name: "mac".into(),
            },
            workspaces: vec![protocol::Workspace {
                id: "w".into(),
                name: "repo".into(),
                path: "/repo".into(),
                topics,
            }],
            approvals: vec![],
            providers: vec![],
            drafts: BTreeMap::new(),
            draft_images: BTreeMap::new(),
            commands: BTreeMap::new(),
            settings: protocol::DeviceSettings::default(),
            awake_until_ms: None,
            mcp_servers: servers,
            paired_clients: vec![],
            pairing_requests: vec![],
        }
    }

    fn topic(id: &str, mcp: &[(&str, bool)]) -> Topic {
        Topic {
            id: id.into(),
            name: id.to_owned(),
            worktree_id: "wt".into(),
            worktrees: vec![],
            sessions: vec![],
            terminals: vec![],
            mcp: mcp
                .iter()
                .map(|(id, on)| (McpServerId::from(*id), *on))
                .collect(),
        }
    }

    #[test]
    fn finds_device_and_topic_servers_by_name_apart() {
        let mut own = server("github", Some("t1"), true, McpAuth::None);
        own.id = McpServerId::from("own");
        let state = state(
            vec![server("github", None, true, McpAuth::None), own],
            vec![],
        );
        let id = |server: Option<&McpServer>| server.map(|s| s.id.0.clone());
        assert_eq!(
            id(device_server(&state, "github")).as_deref(),
            Some("github")
        );
        assert_eq!(
            id(topic_server(&state, &TopicId::from("t1"), "github")).as_deref(),
            Some("own")
        );
        assert_eq!(
            id(topic_server(&state, &TopicId::from("t2"), "github")),
            None
        );
    }

    #[test]
    fn status_lines_show_device_servers_and_topics_that_differ() {
        let state = state(
            vec![
                server("github", None, true, McpAuth::LoggedIn),
                server("fs", None, false, McpAuth::None),
                server("notes", Some("own"), true, McpAuth::None),
            ],
            vec![],
        );
        assert_eq!(
            status_line(&state).as_deref(),
            Some("mcp github on logged in, fs off")
        );
        assert_eq!(topic_status_line(&state, &topic("plain", &[])), None);
        assert_eq!(
            topic_status_line(&state, &topic("own", &[])).as_deref(),
            Some("mcp github, notes")
        );
        assert_eq!(
            topic_status_line(&state, &topic("flipped", &[("github", false)])).as_deref(),
            Some("mcp nothing")
        );
    }
}
