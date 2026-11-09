//! Keeps the machine from sleeping while it's idle and an agent works, so locking the
//! screen and walking away doesn't freeze a turn halfway through.
//!
//! It runs a helper rather than asking the OS directly, because the macOS call would need
//! `unsafe`. The helper only holds off idle sleep, so closing a laptop's lid still sleeps.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;

/// The command that keeps the machine awake for as long as it runs.
#[derive(Debug, Clone)]
pub struct KeepAwakeCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl KeepAwakeCommand {
    /// `caffeinate` on macOS. `-w` ends it with the daemon, even if the daemon is killed
    /// before it can stop it. Elsewhere there's nothing yet, so machines sleep as usual.
    pub fn system() -> Option<Self> {
        cfg!(target_os = "macos").then(|| Self {
            program: "/usr/bin/caffeinate".into(),
            args: vec![
                "-i".into(),
                "-w".into(),
                std::process::id().to_string().into(),
            ],
        })
    }
}

pub struct KeepAwake {
    command: Option<KeepAwakeCommand>,
    running: Option<tokio::process::Child>,
}

impl KeepAwake {
    pub fn new(command: Option<KeepAwakeCommand>) -> Self {
        Self {
            command,
            running: None,
        }
    }

    /// Starts or stops the helper. Failing to start it only costs sleep prevention, so
    /// it's logged and tried again on the next change.
    pub fn set(&mut self, awake: bool) {
        if self
            .running
            .as_mut()
            .is_some_and(|child| child.try_wait().is_ok_and(|status| status.is_some()))
        {
            tracing::warn!("the keep awake helper stopped by itself");
            self.running = None;
        }
        if !awake {
            // Dropping kills it, and tokio reaps it in the background.
            self.running = None;
            return;
        }
        let Some(command) = self.command.as_ref().filter(|_| self.running.is_none()) else {
            return;
        };
        match tokio::process::Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => self.running = Some(child),
            Err(error) => tracing::warn!(
                "couldn't run {} to keep this machine awake: {error}",
                command.program.display()
            ),
        }
    }

    #[cfg(test)]
    pub fn is_on(&self) -> bool {
        self.running.is_some()
    }
}
