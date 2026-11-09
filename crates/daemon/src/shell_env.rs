//! The environment of the person's login shell. launchd and the Dock start
//! programs with a bare `PATH`, so a daemon started that way wouldn't find
//! agents installed through nvm, `~/.local/bin` and the like, or see variables
//! the person exports for them.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::process::Command;

/// Separate the environment from whatever the shell's startup and logout files print.
const START: &[u8] = b"__TECTR_ENV_START__";
const END: &[u8] = b"__TECTR_ENV_END__";

/// Long enough for a slow nvm setup, short enough that a startup file waiting on
/// input can't stop the daemon from starting.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Runs `shell` as an interactive login shell, because that's where people set
/// up `PATH` (nvm, for one, only loads in interactive shells), and returns the
/// environment it ends up with.
pub async fn capture(shell: &Path, timeout: Duration) -> Result<BTreeMap<OsString, OsString>> {
    let script = format!(
        "printf '%s' {}; /usr/bin/env -0; printf '%s' {}",
        String::from_utf8_lossy(START),
        String::from_utf8_lossy(END)
    );
    let child = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to start {}", shell.display()))?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .with_context(|| format!("{} didn't finish within {timeout:?}", shell.display()))??;
    parse(&output.stdout)
}

fn parse(stdout: &[u8]) -> Result<BTreeMap<OsString, OsString>> {
    let find = |marker: &[u8], from: usize| {
        stdout[from..]
            .windows(marker.len())
            .position(|window| window == marker)
            .map(|at| from + at)
    };
    let Some(start) = find(START, 0).map(|at| at + START.len()) else {
        bail!("the shell didn't print its environment");
    };
    let Some(end) = find(END, start) else {
        bail!("the shell's environment was cut short");
    };
    let vars: BTreeMap<_, _> = stdout[start..end]
        .split(|&byte| byte == 0)
        .filter_map(|entry| {
            let equals = entry.iter().position(|&byte| byte == b'=')?;
            Some((
                OsString::from_vec(entry[..equals].to_vec()),
                OsString::from_vec(entry[equals + 1..].to_vec()),
            ))
        })
        .filter(|(name, _)| !name.is_empty())
        .collect();
    if !vars.contains_key(OsString::from("PATH").as_os_str()) {
        bail!("the shell's environment has no PATH");
    }
    Ok(vars)
}

/// The person's shell, falling back to the macOS default when launchd doesn't
/// say.
pub fn login_shell() -> OsString {
    std::env::var_os("SHELL")
        .filter(|shell| !shell.as_bytes().is_empty())
        .unwrap_or_else(|| "/bin/zsh".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_what_startup_files_print() {
        let mut stdout = b"Welcome!\n".to_vec();
        stdout.extend_from_slice(START);
        stdout.extend_from_slice(b"PATH=/a:/b\0EMPTY=\0WITH_EQUALS=a=b\0");
        stdout.extend_from_slice(END);
        stdout.extend_from_slice(b"Bye=now\n");
        let vars = parse(&stdout).unwrap();
        assert_eq!(vars[&OsString::from("PATH")], "/a:/b");
        assert_eq!(vars[&OsString::from("EMPTY")], "");
        assert_eq!(vars[&OsString::from("WITH_EQUALS")], "a=b");
        assert_eq!(vars.len(), 3);
    }

    #[test]
    fn values_can_hold_newlines() {
        let mut stdout = START.to_vec();
        stdout.extend_from_slice(b"PATH=/a\0MULTI=one\ntwo\0");
        stdout.extend_from_slice(END);
        assert_eq!(
            parse(&stdout).unwrap()[&OsString::from("MULTI")],
            "one\ntwo"
        );
    }

    #[test]
    fn refuses_output_without_the_environment() {
        assert!(parse(b"PATH=/a\0").is_err());
        let mut cut_short = START.to_vec();
        cut_short.extend_from_slice(b"PATH=/a\0");
        assert!(parse(&cut_short).is_err());
        let mut no_path = START.to_vec();
        no_path.extend_from_slice(b"HOME=/h\0");
        no_path.extend_from_slice(END);
        assert!(parse(&no_path).is_err());
    }

    #[tokio::test]
    async fn captures_a_real_shell() {
        let vars = capture(Path::new("/bin/sh"), TIMEOUT).await.unwrap();
        assert!(vars.contains_key(OsString::from("PATH").as_os_str()));
    }
}
