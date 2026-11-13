//! `tectr terminal`: shells in a topic's folder, opened, read and typed into without showing
//! them, so agents can use them too.

use std::time::Duration;

use anyhow::{Result, bail};
use clap::Subcommand;
use engine::Engine;
use protocol::{Command, CommandOutput, MachineId, TerminalId};

use crate::{resolve, resolve_topic, short};

/// A terminal no app shows yet takes this size, and the first app to show it gives it its own.
const SIZE: (u16, u16) = (120, 40);
/// How often `wait` reads the terminal.
const POLL: Duration = Duration::from_millis(200);

const KEYS_HELP: &str = "Keys: enter, tab, esc, space, backspace, delete, up, down, left, right, \
                         home, end, pageup, pagedown, and ctrl-A to ctrl-Z, such as ctrl-c.";

#[derive(Subcommand)]
pub enum TerminalCmd {
    /// Open a terminal tab in a topic's folder and print its ID. The apps show it, but don't
    /// switch to it.
    Open {
        /// The topic, by name or ID prefix. Inside a tectr agent, its own topic.
        #[arg(long, env = "TECTR_TOPIC")]
        topic: String,
        /// A command to run in it, typed as if at its prompt, such as a dev server.
        #[arg(long, value_name = "COMMAND")]
        run: Option<String>,
    },
    /// List a topic's terminals, or every terminal without a topic.
    Ls {
        /// The topic, by name or ID prefix. Inside a tectr agent, its own topic.
        #[arg(long, env = "TECTR_TOPIC")]
        topic: Option<String>,
    },
    /// Print what a terminal shows as plain text, with wrapped lines joined.
    Read {
        /// The terminal's ID or a prefix of it.
        terminal: String,
        /// How many of its last lines, history included. Without it, as many as it has rows.
        #[arg(long)]
        lines: Option<u32>,
    },
    /// Type text into a terminal and press Enter.
    Send {
        /// The terminal's ID or a prefix of it.
        terminal: String,
        text: String,
        /// Type it without pressing Enter.
        #[arg(long)]
        no_enter: bool,
    },
    /// Press keys in a terminal, one after another.
    #[command(after_help = KEYS_HELP)]
    Key {
        /// The terminal's ID or a prefix of it.
        terminal: String,
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Wait until a terminal's last lines match a regular expression, then print them. Fails
    /// after the timeout, printing them anyway.
    Wait {
        /// The terminal's ID or a prefix of it.
        terminal: String,
        pattern: String,
        /// Seconds to wait.
        #[arg(long, default_value_t = 60)]
        timeout: u64,
        /// How many of its last lines to match, history included. Without it, as many as it
        /// has rows.
        #[arg(long)]
        lines: Option<u32>,
    },
    /// Stop a terminal's shell and close its tab.
    Close {
        /// The terminal's ID or a prefix of it.
        terminal: String,
    },
}

pub async fn run(engine: &Engine, command: TerminalCmd) -> Result<()> {
    match command {
        TerminalCmd::Open { topic, run } => {
            let (machine, topic_id) = resolve_topic(engine, &topic)?;
            let output = engine
                .request(
                    &machine,
                    Command::OpenTerminal {
                        topic_id,
                        cols: SIZE.0,
                        rows: SIZE.1,
                    },
                )
                .await?;
            let CommandOutput::Terminal { terminal, .. } = output else {
                bail!("the daemon didn't open a terminal");
            };
            if let Some(run) = run {
                write(engine, &machine, &terminal.id, format!("{run}\r")).await?;
            }
            println!("terminal {}", short(&terminal.id.0));
        }
        TerminalCmd::Ls { topic } => {
            let topic = topic
                .map(|topic| resolve_topic(engine, &topic))
                .transpose()?;
            for device in engine.devices() {
                let Some(state) = device.state else { continue };
                for each in state.topics() {
                    if topic.as_ref().is_some_and(|(_, id)| id != &each.id) {
                        continue;
                    }
                    for terminal in &each.terminals {
                        println!(
                            "terminal {} {:?} topic {} {}",
                            short(&terminal.id.0),
                            terminal.status,
                            each.name,
                            terminal.title.as_deref().unwrap_or(""),
                        );
                    }
                }
            }
        }
        TerminalCmd::Read { terminal, lines } => {
            let (machine, id) = resolve_terminal(engine, &terminal)?;
            println!("{}", read(engine, &machine, &id, lines).await?);
        }
        TerminalCmd::Send {
            terminal,
            text,
            no_enter,
        } => {
            let (machine, id) = resolve_terminal(engine, &terminal)?;
            let text = if no_enter { text } else { format!("{text}\r") };
            write(engine, &machine, &id, text).await?;
        }
        TerminalCmd::Key { terminal, keys } => {
            let (machine, id) = resolve_terminal(engine, &terminal)?;
            let text = keys
                .iter()
                .map(|name| key(name))
                .collect::<Result<String>>()?;
            write(engine, &machine, &id, text).await?;
        }
        TerminalCmd::Wait {
            terminal,
            pattern,
            timeout,
            lines,
        } => {
            let pattern = regex::Regex::new(&pattern)?;
            let (machine, id) = resolve_terminal(engine, &terminal)?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
            loop {
                let text = read(engine, &machine, &id, lines).await?;
                if pattern.is_match(&text) {
                    println!("{text}");
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    println!("{text}");
                    bail!("the terminal didn't show {pattern} within {timeout}s");
                }
                tokio::time::sleep(POLL).await;
            }
        }
        TerminalCmd::Close { terminal } => {
            let (machine, terminal_id) = resolve_terminal(engine, &terminal)?;
            engine
                .request(&machine, Command::CloseTerminal { terminal_id })
                .await?;
        }
    }
    Ok(())
}

fn resolve_terminal(engine: &Engine, prefix: &str) -> Result<(MachineId, TerminalId)> {
    resolve(
        engine,
        prefix,
        |state| {
            state
                .topics()
                .flat_map(|topic| topic.terminals.iter().map(|t| t.id.clone()))
                .collect()
        },
        |id| &id.0,
    )
}

async fn read(
    engine: &Engine,
    machine: &MachineId,
    terminal_id: &TerminalId,
    lines: Option<u32>,
) -> Result<String> {
    let command = Command::ReadTerminal {
        terminal_id: terminal_id.clone(),
        lines,
    };
    match engine.request(machine, command).await? {
        CommandOutput::TerminalText { text } => Ok(text),
        _ => bail!("the daemon didn't send the terminal's text"),
    }
}

async fn write(
    engine: &Engine,
    machine: &MachineId,
    terminal_id: &TerminalId,
    text: String,
) -> Result<()> {
    let command = Command::WriteTerminal {
        terminal_id: terminal_id.clone(),
        text,
    };
    engine.request(machine, command).await?;
    Ok(())
}

/// What a terminal gets when a key is pressed, as most programs expect it.
fn key(name: &str) -> Result<String> {
    let lower = name.to_ascii_lowercase();
    let sequence = match lower.as_str() {
        "enter" | "return" => "\r",
        "tab" => "\t",
        "esc" | "escape" => "\x1b",
        "space" => " ",
        "backspace" => "\x7f",
        "delete" => "\x1b[3~",
        "up" => "\x1b[A",
        "down" => "\x1b[B",
        "right" => "\x1b[C",
        "left" => "\x1b[D",
        "home" => "\x1b[H",
        "end" => "\x1b[F",
        "pageup" => "\x1b[5~",
        "pagedown" => "\x1b[6~",
        _ => {
            if let Some(letter) = lower.strip_prefix("ctrl-")
                && let [byte @ b'a'..=b'z'] = letter.as_bytes()
            {
                return Ok(char::from(byte & 0x1f).to_string());
            }
            bail!("no key called {name}. {KEYS_HELP}");
        }
    };
    Ok(sequence.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_become_what_a_terminal_sends() {
        assert_eq!(key("Enter").unwrap(), "\r");
        assert_eq!(key("ctrl-c").unwrap(), "\x03");
        assert_eq!(key("CTRL-D").unwrap(), "\x04");
        assert_eq!(key("up").unwrap(), "\x1b[A");
        assert!(key("ctrl-1").is_err());
        assert!(key("hyper").is_err());
    }
}
