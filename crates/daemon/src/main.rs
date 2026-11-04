use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use daemon::{AgentLauncher, Config, Daemon};
use protocol::DEFAULT_PORT;
use tracing_subscriber::EnvFilter;

/// Runs tectr on this machine. It listens on loopback only.
#[derive(Parser)]
#[command(name = "tectr-daemon", version)]
struct Args {
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,

    /// Where the database, worktrees, and adapters live.
    #[arg(long, env = "TECTR_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Add an ACP agent as NAME=COMMAND, with arguments separated by spaces.
    #[arg(long = "agent", value_name = "NAME=COMMAND")]
    agents: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let data_dir = match args.data_dir {
        Some(dir) => dir,
        None => dirs::data_dir()
            .context("couldn't find a data directory. Use --data-dir")?
            .join("tectr"),
    };

    let mut config = Config::new(data_dir, args.port);
    for agent in &args.agents {
        let (name, launcher) = parse_agent(agent)?;
        config.agents.insert(name, launcher);
    }

    let daemon = Daemon::start(config).await?;
    tracing::info!(
        addr = %daemon.addr(),
        machine_id = %daemon.machine_id(),
        "daemon is running"
    );

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");
    daemon.shutdown().await;
    Ok(())
}

fn parse_agent(text: &str) -> Result<(String, AgentLauncher)> {
    let Some((name, command)) = text.split_once('=') else {
        bail!("expected NAME=COMMAND, got {text}");
    };
    let mut words = command.split_whitespace();
    let Some(program) = words.next() else {
        bail!("the agent {name} has an empty command");
    };
    Ok((
        name.to_owned(),
        AgentLauncher::Command {
            program: program.into(),
            args: words.map(str::to_owned).collect(),
        },
    ))
}
