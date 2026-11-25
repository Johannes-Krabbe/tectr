//! Restarts this machine's daemon through launchd, the only thing that runs it in a
//! release. The CLI otherwise only talks to daemons, but a daemon can't restart itself
//! onto a new binary or a changed login shell.

use std::net::{SocketAddr, TcpStream};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use protocol::Channel;

/// The label the Homebrew cask registers the daemon's LaunchAgent under.
const LABEL: &str = "dev.tectr.daemon";

/// Covers the daemon reading the login shell's environment, which may take 10 seconds.
const READY_TIMEOUT: Duration = Duration::from_secs(20);

pub async fn run() -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("restarting the daemon needs launchd, so it only works on macOS");
    }
    if Channel::current() == Channel::Dev {
        bail!("a dev daemon isn't a LaunchAgent: stop `mise run daemon` and start it again");
    }
    let uid = output(Command::new("id").arg("-u"))?;
    let service = format!("gui/{uid}/{LABEL}");
    // -k stops the running daemon first, which waits for it to stop its agents. launchd
    // runs one instance per label, so once -p prints a pid the old one is gone and the
    // port can only be the new one's.
    let pid = output(Command::new("launchctl").args(["kickstart", "-kp", &service]))
        .with_context(|| format!("couldn't restart {LABEL}; is tectr installed with Homebrew?"))?;
    let pid = pid.split_whitespace().last().unwrap_or_default().to_owned();
    wait_until_listening().await?;
    println!("restarted the daemon, pid {pid}");
    Ok(())
}

async fn wait_until_listening() -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], Channel::current().default_port()));
    let started = Instant::now();
    while TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
        if started.elapsed() > READY_TIMEOUT {
            bail!(
                "the daemon restarted but isn't listening on {addr}; see ~/Library/Logs/tectr/daemon.log"
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(())
}

fn output(command: &mut Command) -> Result<String> {
    let out = command.output()?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
