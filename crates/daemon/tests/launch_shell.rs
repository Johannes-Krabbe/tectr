//! Shells start through the daemon's own binary, so the terminal controls them without the
//! daemon forking. These run that binary the way a terminal does.

use std::fs::File;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

use rustix::fs::{Mode, OFlags};
use rustix::pty::OpenptFlags;

fn open_pty() -> (OwnedFd, OwnedFd) {
    let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
    rustix::pty::grantpt(&master).unwrap();
    rustix::pty::unlockpt(&master).unwrap();
    let name = rustix::pty::ptsname(&master, Vec::new()).unwrap();
    let slave = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY,
        Mode::empty(),
    )
    .unwrap();
    (master, slave)
}

/// Runs a shell command through the launcher on a fresh terminal, returning what it printed.
fn launch(folder: &Path, script: &str) -> (String, ExitStatus) {
    let (master, slave) = open_pty();
    let mut command = Command::new(env!("CARGO_BIN_EXE_tectr-daemon"));
    command
        .arg(daemon::LAUNCH_SHELL)
        .arg(folder)
        .args(["/bin/sh", "-c", script])
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    let mut child = command.spawn().unwrap();
    drop(command);
    let mut output = vec![];
    // Reading ends with an error once the shell's end closes, after what it printed.
    let _ = File::from(master).read_to_end(&mut output);
    let status = child.wait().unwrap();
    (String::from_utf8_lossy(&output).into_owned(), status)
}

#[test]
fn the_shell_leads_its_own_session_on_the_terminal_in_its_folder() {
    let folder = tempfile::tempdir().unwrap();
    let (output, status) = launch(folder.path(), "ps -o stat= -p $$; pwd -P");
    assert!(status.success(), "{output}");
    let mut lines = output.lines().map(str::trim);
    let state = lines.next().unwrap();
    // `s` leads a session, and `+` is in the foreground of the terminal it controls.
    assert!(state.contains('s') && state.contains('+'), "{output}");
    let folder = folder.path().canonicalize().unwrap();
    assert_eq!(lines.next(), folder.to_str(), "{output}");
}

#[test]
fn a_shell_that_cannot_start_says_why_on_the_terminal() {
    let folder = tempfile::tempdir().unwrap();
    let (output, status) = launch(&folder.path().join("gone"), "true");
    assert_eq!(status.code(), Some(1));
    assert!(output.contains("couldn't start /bin/sh"), "{output}");
}
