use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use daemon::{AgentLauncher, Config, Daemon};
use protocol::Channel;
use tracing_subscriber::EnvFilter;

/// Runs tectr on this machine. It listens on loopback only.
#[derive(Parser)]
#[command(name = "tectr-daemon", version)]
struct Args {
    #[arg(long, default_value_t = Channel::current().default_port())]
    port: u16,

    /// Where the database, worktrees, and adapters live.
    #[arg(long, env = "TECTR_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Add an ACP agent as NAME=COMMAND, with arguments separated by spaces.
    #[arg(long = "agent", value_name = "NAME=COMMAND")]
    agents: Vec<String>,

    /// Start over with the environment of your login shell, for when launchd or
    /// the Dock starts the daemon with a bare PATH that wouldn't find your agents.
    #[cfg(unix)]
    #[arg(long)]
    shell_env: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        // launchd writes this to a log file, where color codes are noise.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    #[cfg(unix)]
    if args.shell_env {
        restart_with_shell_env().await;
    }
    let data_dir = match args.data_dir {
        Some(dir) => dir,
        None => dirs::data_dir()
            .context("couldn't find a data directory. Use --data-dir")?
            .join(Channel::current().dir_name()),
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

    wait_for_stop().await?;
    tracing::info!("shutting down");
    daemon.shutdown().await;
    Ok(())
}

/// Ctrl-C in a terminal, or SIGTERM from launchd when the service stops or is upgraded.
async fn wait_for_stop() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result?,
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

/// Replaces this process with itself, minus `--shell-env`, in the login shell's
/// environment. Starting over is what lets agents and everything else see that
/// environment without changing this process's own, which isn't safe once threads run.
/// If the shell can't be read, the daemon carries on with what it has.
#[cfg(unix)]
async fn restart_with_shell_env() {
    use std::os::unix::process::CommandExt;

    let shell = daemon::shell_env::login_shell();
    let vars = match daemon::shell_env::capture(shell.as_ref(), daemon::shell_env::TIMEOUT).await {
        Ok(vars) => vars,
        Err(error) => {
            tracing::warn!(
                "couldn't read your shell's environment, so agents may not be found: {error:#}"
            );
            return;
        }
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            tracing::warn!("couldn't restart with your shell's environment: {error}");
            return;
        }
    };
    let args = std::env::args_os()
        .skip(1)
        .filter(|arg| arg != "--shell-env");
    let error = std::process::Command::new(exe)
        .args(args)
        .env_clear()
        .envs(vars)
        .exec();
    tracing::warn!("couldn't restart with your shell's environment: {error}");
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
