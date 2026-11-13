//! What tells an agent it runs in tectr: its session and topic in its environment, this
//! daemon's CLI on its PATH, and a few lines of instructions that say what the CLI is for.
//! The CLI's own help has the rest, so the instructions stay short.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use protocol::{Channel, SessionId, TopicId};

/// The CLI as agents get it: a folder holding only the CLI, under this channel's name.
#[derive(Debug, Clone)]
pub struct AgentCli {
    folder: PathBuf,
    name: &'static str,
}

impl AgentCli {
    /// Links `cli` into `<data_dir>/bin` as this channel's CLI. A link rather than a copy, so
    /// a rebuilt CLI is what agents get.
    pub fn install(data_dir: &Path, cli: &Path) -> Result<Self> {
        let name = Channel::current().cli_name();
        let folder = data_dir.join("bin");
        std::fs::create_dir_all(&folder)
            .with_context(|| format!("couldn't make {}", folder.display()))?;
        let link = folder.join(name);
        match std::fs::remove_file(&link) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("couldn't replace {}", link.display()));
            }
        }
        std::os::unix::fs::symlink(cli, &link)
            .with_context(|| format!("couldn't link {}", link.display()))?;
        Ok(Self { folder, name })
    }

    /// The daemon's PATH with the CLI's folder first.
    fn path(&self) -> String {
        let rest = std::env::var_os("PATH").unwrap_or_default();
        let mut folders = vec![self.folder.clone()];
        folders.extend(std::env::split_paths(&rest));
        std::env::join_paths(folders)
            .unwrap_or_else(|_| OsString::from(&self.folder))
            .to_string_lossy()
            .into_owned()
    }
}

/// What one agent process gets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentContext {
    pub env: Vec<(String, String)>,
    /// Added to the agent's own system prompt. None without a CLI to tell it about.
    pub instructions: Option<String>,
}

impl AgentContext {
    pub fn new(cli: Option<&AgentCli>, session: &SessionId, topic: &TopicId) -> Self {
        let mut env = vec![
            ("TECTR_SESSION".to_owned(), session.0.clone()),
            ("TECTR_TOPIC".to_owned(), topic.0.clone()),
        ];
        let Some(cli) = cli else {
            return Self {
                env,
                instructions: None,
            };
        };
        env.push(("PATH".to_owned(), cli.path()));
        Self {
            env,
            instructions: Some(instructions(cli.name, session, topic)),
        }
    }
}

fn instructions(cli: &str, session: &SessionId, topic: &TopicId) -> String {
    format!(
        "You are running inside tectr, an app for running many coding agents in parallel. \
         Its work is organized as device -> workspace (a git repository) -> topic (one task, \
         with its own branch and worktree) -> tabs (agent sessions like you, and terminals).

You are session {session} in topic {topic}, also in $TECTR_SESSION and $TECTR_TOPIC. Use \
         the `{cli}` command to work with tectr. Its topic arguments default to your topic.
- `{cli} status`: devices, workspaces, topics, sessions and terminals
- `{cli} terminal open [--run COMMAND]`: open a terminal tab in your topic that the user \
         can see too, for example for a dev server
- `{cli} terminal read|send|key|wait|close TERMINAL`: see what a terminal shows and type \
         into it
- `{cli} new-session`, `{cli} prompt SESSION TEXT`, `{cli} watch SESSION`: start other \
         agents and talk to them
Run `{cli} help` or `{cli} help COMMAND` for the other commands and for details."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_is_linked_under_the_channels_name_and_comes_first_on_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("built").join("tectr");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(&cli, "").unwrap();
        // Installing again replaces the link, as every daemon start does.
        AgentCli::install(dir.path(), Path::new("/old/tectr")).unwrap();
        let installed = AgentCli::install(dir.path(), &cli).unwrap();
        let link = dir.path().join("bin").join("tectr-dev");
        assert_eq!(std::fs::read_link(&link).unwrap(), cli);

        let context = AgentContext::new(
            Some(&installed),
            &SessionId::from("s1"),
            &TopicId::from("t1"),
        );
        let path = &context.env.iter().find(|(key, _)| key == "PATH").unwrap().1;
        assert_eq!(
            std::env::split_paths(path).next(),
            Some(dir.path().join("bin"))
        );
        let instructions = context.instructions.unwrap();
        assert!(instructions.contains("session s1 in topic t1"));
        assert!(instructions.contains("`tectr-dev help COMMAND`"));
    }

    #[test]
    fn without_a_cli_an_agent_still_knows_where_it_is_but_gets_no_instructions() {
        let context = AgentContext::new(None, &SessionId::from("s1"), &TopicId::from("t1"));
        assert_eq!(
            context.env,
            vec![
                ("TECTR_SESSION".to_owned(), "s1".to_owned()),
                ("TECTR_TOPIC".to_owned(), "t1".to_owned()),
            ]
        );
        assert_eq!(context.instructions, None);
    }
}
