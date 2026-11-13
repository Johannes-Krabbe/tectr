//! Terminals: a shell on a pseudo terminal, and Ghostty's engine keeping what it shows.
//!
//! Each terminal has its own thread, because Ghostty's terminal can't move between threads,
//! plus one that reads the shell's output and one that writes what's typed. Writing never
//! happens on the terminal's thread, so a program that stops reading can't stall its output.
//!
//! Clients get the screen as it is when they attach and then what's printed, and say how
//! much they've drawn. One that falls too far behind is sent nothing more until it catches
//! up, and then the screen as it is by then, so a flood of output never queues up anywhere.

use std::cell::RefCell;
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use libghostty_vt::Terminal;
use libghostty_vt::fmt::{Extras, Format, Formatter};
use libghostty_vt::screen::Screen;
use libghostty_vt::snapshot::Decoder;
use libghostty_vt::style::RgbColor;
use protocol::frame::{self, Kind};
use protocol::{TerminalColors, TerminalId};
use rustix::fs::{Mode, OFlags};
use rustix::io::FdFlags;
use rustix::process::{Pid, Signal};
use rustix::pty::OpenptFlags;
use rustix::termios::Winsize;
use tokio::sync::mpsc::UnboundedSender;

/// Lines of history a terminal keeps.
const SCROLLBACK_LINES: usize = 10_000;
/// Output a client can have unacknowledged before it's sent nothing more.
const MAX_UNACKED: usize = 4 * 1024 * 1024;
/// How far a client that fell behind has to catch up before it gets the screen again.
const RESUME_UNACKED: usize = 512 * 1024;
/// History is saved once output has been quiet this long.
const SAVE_WHEN_QUIET: Duration = Duration::from_secs(2);
/// Or this long after it changed, for output that never goes quiet.
const SAVE_AT_LEAST_EVERY: Duration = Duration::from_secs(30);
/// Output arriving together is drawn together, up to this much.
const BATCH: usize = 256 * 1024;

/// What starts in a terminal.
#[derive(Debug, Clone)]
pub struct Shell {
    pub program: String,
    pub args: Vec<String>,
    /// The daemon's own binary, which starts the shell through `launch_shell` so the
    /// terminal controls it. Without it the shell has no job control, so Ctrl-C reaches
    /// nothing, which only tests get away with.
    pub launcher: Option<PathBuf>,
}

impl Shell {
    /// The person's own shell as a login shell, as a terminal app would start it.
    pub fn login() -> Self {
        let program = std::env::var("SHELL")
            .ok()
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| "/bin/zsh".to_owned());
        Self {
            program,
            args: vec!["-l".to_owned()],
            launcher: std::env::current_exe().ok(),
        }
    }
}

/// The argument that makes the daemon's binary run `launch_shell` instead.
pub const LAUNCH_SHELL: &str = "--launch-shell";

/// Becomes a terminal's shell, given the folder, the program and its arguments. It runs in a
/// process of its own with the terminal as its stdio, and makes a session the terminal
/// controls first. Returns only if that fails.
///
/// The daemon can't do this between fork and exec, the usual way: once Apple's Network
/// framework has done its work in a process, its fork handler crashes the child. Starting
/// this process doesn't fork.
pub fn launch_shell(mut args: impl Iterator<Item = OsString>) -> anyhow::Error {
    let (Some(folder), Some(program)) = (args.next(), args.next()) else {
        return anyhow::anyhow!("expected a folder and a program");
    };
    if let Err(error) = rustix::process::setsid() {
        return anyhow::Error::new(error).context("couldn't start a session");
    }
    if let Err(error) = rustix::process::ioctl_tiocsctty(std::io::stdin()) {
        return anyhow::Error::new(error).context("couldn't take the terminal");
    }
    let error = Command::new(&program).args(args).current_dir(folder).exec();
    anyhow::Error::new(error).context(format!("couldn't start {}", program.display()))
}

pub struct Start {
    pub id: TerminalId,
    pub shell: Shell,
    /// Where the shell starts. `fallback` is used when that's gone, such as a removed folder.
    pub cwd: PathBuf,
    pub fallback: PathBuf,
    pub cols: u16,
    pub rows: u16,
    /// What the terminal showed before, kept across restarts. It's read when the terminal
    /// starts, and written as it changes.
    pub history: PathBuf,
}

/// What a terminal tells the daemon. These come from the terminal's own thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Running,
    Title(Option<String>),
    /// The shell exited by itself, with its code if it had one.
    Exited(Option<u32>),
    /// Where the shell is, for the next one to start in.
    Cwd(String),
}

pub type Notify = Box<dyn Fn(Event) + Send>;

/// A client showing the terminal. Frames are ready to send as binary websocket messages.
pub struct Subscriber {
    pub id: u64,
    pub frames: UnboundedSender<Vec<u8>>,
}

enum Message {
    Output(Vec<u8>),
    ShellExited {
        run: u64,
        status: Option<ExitStatus>,
    },
    Input(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    Attach {
        subscriber: Subscriber,
        cols: u16,
        rows: u16,
        colors: Option<TerminalColors>,
    },
    Detach(u64),
    Ack {
        subscriber: u64,
        bytes: u32,
    },
    /// What it shows as plain text, for clients that don't draw it.
    Read {
        lines: Option<u32>,
        reply: tokio::sync::oneshot::Sender<String>,
    },
    /// Ends the terminal. Kept history is saved first, and otherwise deleted.
    Stop {
        keep: bool,
    },
}

pub struct Handle {
    messages: Sender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn start(start: Start, notify: Notify) -> Result<Self> {
        let (messages, receiver) = mpsc::channel();
        let own = messages.clone();
        let name = format!("terminal-{}", &start.id.0[..start.id.0.len().min(8)]);
        let thread = std::thread::Builder::new()
            .name(name)
            .spawn(move || run(start, receiver, own, notify))
            .context("failed to start a terminal's thread")?;
        Ok(Self {
            messages,
            thread: Some(thread),
        })
    }

    pub fn input(&self, data: Vec<u8>) {
        let _ = self.messages.send(Message::Input(data));
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.messages.send(Message::Resize { cols, rows });
    }

    pub fn attach(
        &self,
        subscriber: Subscriber,
        cols: u16,
        rows: u16,
        colors: Option<TerminalColors>,
    ) {
        let _ = self.messages.send(Message::Attach {
            subscriber,
            cols,
            rows,
            colors,
        });
    }

    pub fn detach(&self, subscriber: u64) {
        let _ = self.messages.send(Message::Detach(subscriber));
    }

    pub fn ack(&self, subscriber: u64, bytes: u32) {
        let _ = self.messages.send(Message::Ack { subscriber, bytes });
    }

    /// Its last `lines` lines as plain text, or as many as it has rows. Fails only if the
    /// terminal ended first.
    /// The answer doesn't borrow the handle, so it can be awaited after letting go of it.
    pub fn read(&self, lines: Option<u32>) -> impl Future<Output = Result<String>> + use<> {
        let (reply, text) = tokio::sync::oneshot::channel();
        let _ = self.messages.send(Message::Read { lines, reply });
        async move { text.await.context("the terminal ended") }
    }

    /// Ends the shell, keeping what the terminal showed for next time or deleting it. Returns
    /// the thread, which finishes as soon as that's done.
    pub fn stop(mut self, keep: bool) -> Option<JoinHandle<()>> {
        let _ = self.messages.send(Message::Stop { keep });
        self.thread.take()
    }
}

/// The running shell. Its output is read on its own thread, which also waits for it to exit.
struct ShellProcess {
    run: u64,
    pid: u32,
    /// The terminal's end of the pseudo terminal.
    master: OwnedFd,
}

enum Write_ {
    Data(Vec<u8>),
    /// The shell's end to write to, or none once it's gone so the pseudo terminal can close.
    Pty(Option<File>),
}

struct Client {
    subscriber: Subscriber,
    unacked: usize,
    /// It fell behind, and gets the screen again once it catches up.
    behind: bool,
}

struct Emulator {
    id: TerminalId,
    shell: Shell,
    fallback: PathBuf,
    history: PathBuf,
    cwd: PathBuf,
    cols: u16,
    rows: u16,
    terminal: Terminal<'static, 'static>,
    title: Rc<RefCell<Option<Option<String>>>>,
    process: Option<ShellProcess>,
    runs: u64,
    writer: Sender<Write_>,
    messages: Sender<Message>,
    clients: Vec<Client>,
    /// When what it shows last changed without being saved, and when that started.
    changed: Option<(Instant, Instant)>,
    notify: Notify,
}

fn run(start: Start, messages: Receiver<Message>, own: Sender<Message>, notify: Notify) {
    let mut emulator = match Emulator::new(start, own, notify) {
        Ok(emulator) => emulator,
        Err(error) => {
            tracing::error!("a terminal didn't start: {error:#}");
            return;
        }
    };
    if let Err(error) = emulator.spawn_shell() {
        emulator.show_failure(&format!("{error:#}"));
        (emulator.notify)(Event::Exited(None));
    }
    loop {
        let message = match messages.recv_timeout(Duration::from_secs(1)) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => {
                emulator.save_if_due();
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match message {
            Message::Output(mut data) => {
                // Draw whatever else has already arrived along with it, so a flood is a few
                // large writes rather than many small ones.
                let mut stop = None;
                while data.len() < BATCH {
                    match messages.try_recv() {
                        Ok(Message::Output(more)) => data.extend_from_slice(&more),
                        Ok(other) => {
                            stop = Some(other);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                emulator.output(&data);
                if let Some(other) = stop
                    && emulator.handle(other)
                {
                    break;
                }
            }
            other => {
                if emulator.handle(other) {
                    break;
                }
            }
        }
        emulator.save_if_due();
    }
}

impl Emulator {
    fn new(start: Start, messages: Sender<Message>, notify: Notify) -> Result<Self> {
        let mut terminal = Terminal::new(start.cols, start.rows)
            .map_err(|error| anyhow::anyhow!("Ghostty's terminal failed: {error:?}"))?;
        terminal
            .set_scrollback_max_lines(Some(SCROLLBACK_LINES))
            .and_then(|terminal| terminal.set_scrollback_max_bytes(Some(usize::MAX)))
            .map_err(|error| anyhow::anyhow!("Ghostty's terminal failed: {error:?}"))?;

        let (writer, writes) = mpsc::channel();
        std::thread::Builder::new()
            .name("terminal-writer".to_owned())
            .spawn(move || write_loop(writes))
            .context("failed to start a terminal's writer")?;

        // Programs ask the terminal things, like where the cursor is, and Ghostty answers.
        // Clients don't, so each question is answered once.
        let answers = writer.clone();
        terminal
            .on_pty_write(move |_, data: &[u8]| {
                if !is_kitty_keyboard_report(data) {
                    let _ = answers.send(Write_::Data(data.to_vec()));
                }
            })
            .map_err(|error| anyhow::anyhow!("Ghostty's terminal failed: {error:?}"))?;
        let title = Rc::new(RefCell::new(None));
        let changed_title = title.clone();
        terminal
            .on_title_changed(move |terminal| {
                let text = terminal
                    .title()
                    .map(|title| String::from_utf8_lossy(title).trim().to_owned())
                    .unwrap_or_default();
                *changed_title.borrow_mut() = Some(Some(text).filter(|text| !text.is_empty()));
            })
            .map_err(|error| anyhow::anyhow!("Ghostty's terminal failed: {error:?}"))?;

        let mut emulator = Self {
            id: start.id,
            shell: start.shell,
            fallback: start.fallback,
            history: start.history,
            cwd: start.cwd,
            cols: start.cols,
            rows: start.rows,
            terminal,
            title,
            process: None,
            runs: 0,
            writer,
            messages,
            clients: vec![],
            changed: None,
            notify,
        };
        emulator.restore_history();
        Ok(emulator)
    }

    /// Shows what the terminal showed before the daemon restarted, then a line, then the new
    /// shell. Only text and its colors come back: the modes a program had set left with it.
    fn restore_history(&mut self) {
        let Ok(history) = std::fs::read(&self.history) else {
            return;
        };
        if history.is_empty() {
            return;
        }
        self.terminal.vt_write(&history);
        let line = "-".repeat(usize::from(self.cols));
        self.terminal
            .vt_write(format!("\x1b[0m\r\n\x1b[2m{line}\x1b[0m\r\n").as_bytes());
    }

    /// Returns whether the terminal is finished.
    fn handle(&mut self, message: Message) -> bool {
        match message {
            Message::Output(data) => self.output(&data),
            Message::ShellExited { run, status } => self.shell_exited(run, status),
            Message::Input(data) => {
                if self.process.is_some() {
                    let _ = self.writer.send(Write_::Data(data));
                } else {
                    // A failed shell is replaced by typing. What was typed was for the old one.
                    self.start_again();
                }
            }
            Message::Resize { cols, rows } => self.resize(cols, rows),
            Message::Attach {
                subscriber,
                cols,
                rows,
                colors,
            } => {
                if let Some(colors) = colors {
                    self.set_colors(&colors);
                }
                let resized = (cols, rows) != (self.cols, self.rows);
                if resized {
                    self.resize(cols, rows);
                }
                let mut client = Client {
                    subscriber,
                    unacked: 0,
                    behind: false,
                };
                let reset = self.reset_frame();
                send_reset(&mut client, reset);
                self.clients.push(client);
            }
            Message::Detach(id) => self.clients.retain(|c| c.subscriber.id != id),
            Message::Ack { subscriber, bytes } => {
                let reset = self
                    .clients
                    .iter_mut()
                    .find(|c| c.subscriber.id == subscriber)
                    .is_some_and(|client| {
                        client.unacked = client.unacked.saturating_sub(bytes as usize);
                        client.behind && client.unacked <= RESUME_UNACKED
                    });
                if reset {
                    let frame = self.reset_frame();
                    if let Some(client) = self
                        .clients
                        .iter_mut()
                        .find(|c| c.subscriber.id == subscriber)
                    {
                        client.behind = false;
                        send_reset(client, frame);
                    }
                }
            }
            Message::Read { lines, reply } => {
                let _ = reply.send(self.text(lines));
            }
            Message::Stop { keep } => {
                if keep {
                    self.save();
                } else {
                    let _ = std::fs::remove_file(&self.history);
                }
                self.end_shell();
                return true;
            }
        }
        false
    }

    fn output(&mut self, data: &[u8]) {
        self.terminal.vt_write(data);
        let now = Instant::now();
        self.changed = Some((now, self.changed.map_or(now, |(_, since)| since)));
        if let Some(title) = self.title.borrow_mut().take() {
            (self.notify)(Event::Title(title));
        }
        let frame = frame::encode(Kind::Output, &self.id, data);
        self.clients.retain_mut(|client| {
            if client.behind {
                return true;
            }
            if client.unacked > MAX_UNACKED {
                client.behind = true;
                return true;
            }
            client.unacked += data.len();
            client.subscriber.frames.send(frame.clone()).is_ok()
        });
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 || (cols, rows) == (self.cols, self.rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        if let Err(error) = self.terminal.resize(cols, rows, 0, 0) {
            tracing::warn!("a terminal didn't resize: {error:?}");
        }
        if let Some(process) = &self.process {
            let _ = rustix::termios::tcsetwinsize(&process.master, winsize(cols, rows));
        }
        // Everyone showing it starts over at the new size, so no client draws lines wrapped
        // for another.
        let frame = self.reset_frame();
        for client in &mut self.clients {
            if !client.behind {
                send_reset(client, frame.clone());
            }
        }
    }

    fn set_colors(&mut self, colors: &TerminalColors) {
        let fg = rgb(&colors.foreground);
        let bg = rgb(&colors.background);
        let cursor = rgb(&colors.cursor);
        let mut palette = libghostty_vt::style::Palette::default();
        for (index, color) in colors.palette.iter().take(16).enumerate() {
            if let Some(color) = rgb(color) {
                palette.0[index] = color;
            }
        }
        let result = self
            .terminal
            .set_default_fg_color(fg)
            .and_then(|terminal| terminal.set_default_bg_color(bg))
            .and_then(|terminal| terminal.set_default_cursor_color(cursor))
            .and_then(|terminal| terminal.set_default_color_palette(Some(palette)));
        if let Err(error) = result {
            tracing::warn!("a terminal didn't take the client's colors: {error:?}");
        }
    }

    /// The last `lines` lines of the screen and its history, with wrapped lines joined so a
    /// long line reads as one.
    fn text(&self, lines: Option<u32>) -> String {
        let text = Formatter::new(&self.terminal)
            .ok()
            .and_then(|mut formatter| {
                formatter
                    .set_format(Format::Plain)
                    .and_then(|f| f.set_unwrap(true))
                    .and_then(|f| f.set_trim(true))
                    .and_then(|f| f.set_extras(Extras::NONE))
                    .ok()?;
                Some(formatter.format_alloc(None).ok()?.to_vec())
            })
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&text);
        let all: Vec<&str> = text.lines().collect();
        let wanted = lines.map_or(usize::from(self.rows), |lines| lines as usize);
        all[all.len().saturating_sub(wanted)..].join("\n")
    }

    /// Everything the terminal shows, modes and cursor included, at its size. The palette is
    /// left out so each client keeps its own colors.
    fn reset_frame(&mut self) -> (Vec<u8>, usize) {
        let screen =
            format(&self.terminal, Extras::ALL - Extras::PALETTE, false).unwrap_or_default();
        let payload = frame::reset_payload(self.cols, self.rows, &screen);
        let len = payload.len();
        (frame::encode(Kind::Reset, &self.id, &payload), len)
    }

    fn spawn_shell(&mut self) -> Result<()> {
        self.runs += 1;
        let run = self.runs;
        let cwd = if self.cwd.is_dir() {
            self.cwd.clone()
        } else {
            self.fallback.clone()
        };
        let (master, slave) =
            open_pty(self.cols, self.rows).context("failed to open a pseudo terminal")?;
        // Nothing here may make std fork rather than posix_spawn, see `launch_shell`. A
        // pre-exec hook would.
        let mut command = match &self.shell.launcher {
            Some(launcher) => {
                let mut command = Command::new(launcher);
                command
                    .arg(LAUNCH_SHELL)
                    .arg(&cwd)
                    .arg(&self.shell.program)
                    .args(&self.shell.args);
                command
            }
            None => {
                let mut command = Command::new(&self.shell.program);
                command.args(&self.shell.args).current_dir(&cwd);
                command
            }
        };
        command
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "tectr")
            .env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"))
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start {}", self.shell.program))?;
        // The command held the shell's end, so only the shell has it open now, and reading
        // ends when it exits.
        drop(command);
        let pid = child.id();
        let mut reader = File::from(master.try_clone()?);
        let writer = File::from(master.try_clone()?);
        let _ = self.writer.send(Write_::Pty(Some(writer)));

        let messages = self.messages.clone();
        std::thread::Builder::new()
            .name("terminal-reader".to_owned())
            .spawn(move || {
                let mut buffer = vec![0; 64 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if messages
                                .send(Message::Output(buffer[..read].to_vec()))
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                let status = child.wait().ok();
                let _ = messages.send(Message::ShellExited { run, status });
            })
            .context("failed to start a terminal's reader")?;

        self.cwd = cwd;
        self.process = Some(ShellProcess { run, pid, master });
        (self.notify)(Event::Running);
        Ok(())
    }

    fn shell_exited(&mut self, run: u64, status: Option<ExitStatus>) {
        if self
            .process
            .as_ref()
            .is_none_or(|process| process.run != run)
        {
            return;
        }
        self.process = None;
        let _ = self.writer.send(Write_::Pty(None));
        let _ = std::fs::remove_file(&self.history);
        let signal = status.and_then(|status| status.signal());
        let code = status
            .and_then(|status| status.code())
            .and_then(|code| u32::try_from(code).ok());
        if code != Some(0) {
            tracing::warn!(id = %self.id, ?code, ?signal, "a terminal's shell ended");
            let what = match (signal, code) {
                (Some(signal), _) => format!("The shell was ended by {}", signal_name(signal)),
                (None, Some(code)) => format!("The shell exited with code {code}"),
                (None, None) => "The shell ended".to_owned(),
            };
            self.show_failure(&format!("{what}. Type to start another."));
        }
        (self.notify)(Event::Exited(code));
    }

    fn show_failure(&mut self, message: &str) {
        let text = format!("\x1b[0m\r\n\x1b[2m{message}\x1b[0m\r\n");
        self.output(text.as_bytes());
    }

    fn start_again(&mut self) {
        if let Err(error) = self.spawn_shell() {
            self.show_failure(&format!("{error:#}"));
        }
    }

    /// Asks the shell to end, as closing a terminal window does. Its children get the same
    /// when the pseudo terminal closes behind it. It ends by itself after that, so a shell
    /// writing its history on the way out isn't cut off.
    fn end_shell(&mut self) {
        if let Some(process) = self.process.take()
            && let Some(pid) = i32::try_from(process.pid).ok().and_then(Pid::from_raw)
        {
            let _ = rustix::process::kill_process(pid, Signal::HUP);
        }
    }

    fn save_if_due(&mut self) {
        let Some((last, since)) = self.changed else {
            return;
        };
        if last.elapsed() >= SAVE_WHEN_QUIET || since.elapsed() >= SAVE_AT_LEAST_EVERY {
            self.save();
        }
    }

    /// Writes what the terminal shows, and says where the shell is, for after a restart.
    fn save(&mut self) {
        if let Some(cwd) = self.process.as_ref().and_then(|p| process_cwd(p.pid)) {
            self.cwd = cwd;
            (self.notify)(Event::Cwd(self.cwd.display().to_string()));
        }
        let Some(history) = history(&mut self.terminal) else {
            // Ghostty can only copy a terminal between whole escape sequences. The next
            // chance comes with the next output.
            return;
        };
        if let Err(error) = write_atomically(&self.history, &history) {
            tracing::warn!(path = %self.history.display(), "failed to save a terminal: {error:#}");
            return;
        }
        self.changed = None;
    }
}

/// A pseudo terminal at this size: the terminal's end, then the shell's.
fn open_pty(cols: u16, rows: u16) -> Result<(OwnedFd, OwnedFd)> {
    let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    rustix::io::fcntl_setfd(&master, FdFlags::CLOEXEC)?;
    rustix::pty::grantpt(&master)?;
    rustix::pty::unlockpt(&master)?;
    let name = rustix::pty::ptsname(&master, Vec::new())?;
    let slave = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    rustix::termios::tcsetwinsize(&master, winsize(cols, rows))?;
    Ok((master, slave))
}

fn winsize(cols: u16, rows: u16) -> Winsize {
    Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

fn signal_name(signal: i32) -> String {
    let name = [
        (Signal::HUP, "SIGHUP"),
        (Signal::INT, "SIGINT"),
        (Signal::QUIT, "SIGQUIT"),
        (Signal::ILL, "SIGILL"),
        (Signal::ABORT, "SIGABRT"),
        (Signal::BUS, "SIGBUS"),
        (Signal::KILL, "SIGKILL"),
        (Signal::SEGV, "SIGSEGV"),
        (Signal::TERM, "SIGTERM"),
    ]
    .into_iter()
    .find(|(known, _)| known.as_raw() == signal);
    name.map_or_else(|| format!("signal {signal}"), |(_, name)| name.to_owned())
}

fn send_reset(client: &mut Client, (frame, len): (Vec<u8>, usize)) {
    client.unacked += len;
    let _ = client.subscriber.frames.send(frame);
}

/// Ghostty supports the kitty keyboard protocol, but xterm.js, which turns keys into bytes, doesn't.
/// A program told the terminal supports it would wait for keys in a form that never comes, so the
/// answer to its question is left out, which is how a terminal without it behaves.
fn is_kitty_keyboard_report(data: &[u8]) -> bool {
    data.strip_prefix(b"\x1b[?")
        .and_then(|rest| rest.strip_suffix(b"u"))
        .is_some_and(|flags| flags.iter().all(u8::is_ascii_digit))
}

fn write_loop(writes: Receiver<Write_>) {
    let mut pty: Option<File> = None;
    for write in writes {
        match write {
            Write_::Pty(writer) => pty = writer,
            Write_::Data(data) => {
                if let Some(writer) = &mut pty
                    && (writer.write_all(&data).is_err() || writer.flush().is_err())
                {
                    pty = None;
                }
            }
        }
    }
}

fn format(terminal: &Terminal, extras: Extras, trim: bool) -> Option<Vec<u8>> {
    let mut formatter = Formatter::new(terminal).ok()?;
    formatter.set_format(Format::Vt).ok()?;
    formatter.set_extras(extras).ok()?;
    formatter.set_trim(trim).ok()?;
    Some(formatter.format_alloc(None).ok()?.to_vec())
}

/// The main screen's text and colors with its history. A program on the other screen, like
/// an editor, ends with the shell, so what's under it is what's worth keeping.
fn history(terminal: &mut Terminal<'static, 'static>) -> Option<Vec<u8>> {
    if terminal.active_screen().ok()? == Screen::Primary {
        return format(terminal, Extras::NONE, true);
    }
    let mut snapshot = vec![];
    terminal.encode_snapshot(&mut snapshot).ok()?;
    let mut copy = Decoder::new_buf(&snapshot).ok()?.decode().ok()?;
    copy.vt_write(b"\x1b[?1049l");
    format(&copy, Extras::NONE, true)
}

fn write_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, contents)?;
    std::fs::rename(&partial, path)?;
    Ok(())
}

fn rgb(hex: &str) -> Option<RgbColor> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
    Some(RgbColor {
        r: channel(0)?,
        g: channel(2)?,
        b: channel(4)?,
    })
}

/// Where a process is. macOS has no file for this, and reading it directly would need code
/// this project keeps out, so `lsof` is asked. It only runs when a terminal is saved.
fn process_cwd(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let output = std::process::Command::new("lsof")
            .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
            .output()
            .ok()?;
        String::from_utf8(output.stdout)
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix('n'))
            .map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    const WAIT: Duration = Duration::from_secs(10);

    struct Running {
        handle: Handle,
        frames: UnboundedReceiver<Vec<u8>>,
        events: Receiver<Event>,
        /// Everything the terminal printed since the last reset, as text.
        seen: String,
        resets: Vec<(u16, u16, String)>,
        received: usize,
    }

    fn start(folder: &Path, history: &Path, cols: u16) -> Running {
        let (events_tx, events) = mpsc::channel();
        let notify: Notify = Box::new(move |event| {
            let _ = events_tx.send(event);
        });
        let handle = Handle::start(
            Start {
                id: TerminalId::from("t1"),
                shell: Shell {
                    program: "/bin/sh".into(),
                    args: vec![],
                    launcher: None,
                },
                cwd: folder.to_owned(),
                fallback: folder.to_owned(),
                cols,
                rows: 24,
                history: history.to_owned(),
            },
            notify,
        )
        .unwrap();
        let (tx, frames) = unbounded_channel();
        handle.attach(Subscriber { id: 1, frames: tx }, cols, 24, None);
        Running {
            handle,
            frames,
            events,
            seen: String::new(),
            resets: vec![],
            received: 0,
        }
    }

    impl Running {
        fn read(&mut self, ack: bool) -> bool {
            let deadline = Instant::now() + Duration::from_millis(50);
            let mut any = false;
            while Instant::now() < deadline {
                let Ok(data) = self.frames.try_recv() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                any = true;
                let frame = frame::decode(&data).unwrap();
                self.received += frame.payload.len();
                match frame.kind {
                    Kind::Output => self.seen += &String::from_utf8_lossy(frame.payload),
                    Kind::Reset => {
                        let (cols, rows, screen) = frame::read_reset(frame.payload).unwrap();
                        let screen = String::from_utf8_lossy(screen).into_owned();
                        self.seen = screen.clone();
                        self.resets.push((cols, rows, screen));
                    }
                    _ => panic!("a terminal only sends output and resets"),
                }
                if ack {
                    self.handle.ack(1, frame.payload.len() as u32);
                }
            }
            any
        }

        fn wait_for(&mut self, text: &str) {
            let deadline = Instant::now() + WAIT;
            while !self.seen.contains(text) {
                assert!(
                    Instant::now() < deadline,
                    "never saw {text:?} in {:?}",
                    self.seen
                );
                self.read(true);
            }
        }

        fn wait_for_event(&mut self, wanted: &Event) {
            let deadline = Instant::now() + WAIT;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                match self.events.recv_timeout(left) {
                    Ok(event) if &event == wanted => return,
                    Ok(_) => {}
                    Err(_) => panic!("never got {wanted:?}"),
                }
            }
        }

        fn type_text(&self, text: &str) {
            self.handle.input(text.as_bytes().to_vec());
        }
    }

    #[test]
    fn a_client_gets_the_screen_then_what_the_shell_prints() {
        let folder = tempfile::tempdir().unwrap();
        let mut terminal = start(folder.path(), &folder.path().join("h.vt"), 80);
        terminal.wait_for_event(&Event::Running);
        terminal.read(true);
        assert_eq!(terminal.resets.first().map(|r| (r.0, r.1)), Some((80, 24)));

        terminal.type_text("echo one-$((40+2))\n");
        terminal.wait_for("one-42");
        terminal.handle.stop(false).unwrap().join().unwrap();
    }

    #[test]
    fn reading_gives_the_last_lines_as_text_with_wrapped_lines_joined() {
        let folder = tempfile::tempdir().unwrap();
        let mut terminal = start(folder.path(), &folder.path().join("h.vt"), 20);
        terminal.wait_for_event(&Event::Running);
        terminal.type_text("i=1; while [ $i -le 60 ]; do echo line-$i; i=$((i+1)); done\n");
        terminal.wait_for("line-60");
        terminal.type_text("echo abcdefghij-abcdefghij-abcdefghij-end\n");
        terminal.wait_for("-end");
        terminal.read(true);
        let read = |lines| {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(terminal.handle.read(lines))
                .unwrap()
        };

        // Older than the screen's 24 rows, so only history has it.
        let long = read(Some(40));
        assert!(long.contains("line-30\n"), "{long:?}");
        assert!(!long.contains("line-10\n"), "{long:?}");
        assert!(
            long.contains("\nabcdefghij-abcdefghij-abcdefghij-end"),
            "{long:?}"
        );

        let screen = read(None);
        assert!(screen.lines().count() <= 24, "{screen:?}");
        assert!(!screen.contains("line-30\n"), "{screen:?}");
        terminal.handle.stop(false).unwrap().join().unwrap();
    }

    /// A program that asks where the cursor is gets one answer, from the daemon.
    #[test]
    fn the_terminal_answers_what_programs_ask() {
        let folder = tempfile::tempdir().unwrap();
        let mut terminal = start(folder.path(), &folder.path().join("h.vt"), 80);
        terminal.wait_for_event(&Event::Running);
        // Asks for the cursor position with the terminal raw, then prints the answer readably.
        terminal.type_text(
            "stty raw -echo; printf '\\033[6n'; r=$(dd bs=1 count=6 2>/dev/null); stty sane; \
             printf 'got:%s\\n' \"$r\" | tr -d '\\033'\n",
        );
        terminal.wait_for("got:[");
        terminal.handle.stop(false).unwrap().join().unwrap();
    }

    #[test]
    fn programs_are_not_told_about_keyboard_support_clients_lack() {
        assert!(is_kitty_keyboard_report(b"\x1b[?0u"));
        assert!(is_kitty_keyboard_report(b"\x1b[?15u"));
        assert!(!is_kitty_keyboard_report(b"\x1b[?62;22c"));

        let folder = tempfile::tempdir().unwrap();
        let mut terminal = start(folder.path(), &folder.path().join("h.vt"), 80);
        terminal.wait_for_event(&Event::Running);
        // Asks about the kitty keyboard protocol, then for device attributes, which always get
        // an answer, and prints what came back readably.
        terminal.type_text(
            "stty raw -echo min 0 time 5; printf '\\033[?u\\033[c'; \
             r=$(dd bs=64 count=1 2>/dev/null); stty sane; \
             printf 'got:%s:end\\n' \"$r\" | tr -d '\\033'\n",
        );
        terminal.wait_for(":end");
        let answer = terminal.seen.split("got:").last().unwrap().to_owned();
        assert!(
            answer.contains('c'),
            "device attributes come back: {answer:?}"
        );
        assert!(
            !answer.contains("u:end") && !answer.contains("u["),
            "{answer:?}"
        );
        terminal.handle.stop(false).unwrap().join().unwrap();
    }

    #[test]
    fn history_comes_back_after_a_restart_and_goes_when_closed() {
        let folder = tempfile::tempdir().unwrap();
        let history = folder.path().join("terminals").join("t1.vt");
        let mut terminal = start(folder.path(), &history, 80);
        terminal.wait_for_event(&Event::Running);
        terminal.type_text("echo kept-$((1+1))\n");
        terminal.wait_for("kept-2");
        terminal.handle.stop(true).unwrap().join().unwrap();
        assert!(history.exists());

        let mut again = start(folder.path(), &history, 80);
        again.wait_for_event(&Event::Running);
        again.read(true);
        let (_, _, screen) = again.resets.first().unwrap();
        assert!(screen.contains("kept-2"), "{screen:?}");
        assert!(
            screen.contains(&"-".repeat(80)),
            "a line marks where it restarted"
        );

        again.handle.stop(false).unwrap().join().unwrap();
        assert!(!history.exists());
    }

    #[test]
    fn a_failed_shell_says_so_and_typing_starts_another() {
        let folder = tempfile::tempdir().unwrap();
        let mut terminal = start(folder.path(), &folder.path().join("h.vt"), 80);
        terminal.wait_for_event(&Event::Running);
        terminal.type_text("exit 3\n");
        terminal.wait_for_event(&Event::Exited(Some(3)));
        terminal.wait_for("exited with code 3");

        terminal.type_text("x");
        terminal.wait_for_event(&Event::Running);
        terminal.type_text("exit\n");
        terminal.wait_for_event(&Event::Exited(Some(0)));
        terminal.handle.stop(false).unwrap().join().unwrap();
    }

    #[test]
    fn resizing_starts_every_client_over_at_the_new_size() {
        let folder = tempfile::tempdir().unwrap();
        let mut terminal = start(folder.path(), &folder.path().join("h.vt"), 80);
        terminal.wait_for_event(&Event::Running);
        terminal.handle.resize(100, 30);
        terminal.type_text("stty size\n");
        terminal.wait_for("30 100");
        assert!(terminal.resets.iter().any(|r| (r.0, r.1) == (100, 30)));
        terminal.handle.stop(false).unwrap().join().unwrap();
    }

    /// A client that stops acknowledging is sent a bounded amount, and the screen as it is
    /// once it catches up, however much the shell printed meanwhile.
    #[test]
    fn a_client_that_falls_behind_skips_ahead_instead_of_queueing() {
        let folder = tempfile::tempdir().unwrap();
        let mut terminal = start(folder.path(), &folder.path().join("h.vt"), 80);
        terminal.wait_for_event(&Event::Running);
        terminal.read(true);
        terminal
            .type_text("head -c 40000000 /dev/zero | tr '\\0' 'x'; echo; echo flood-$((6*7))\n");
        // Long enough for the shell to print all of it.
        let deadline = Instant::now() + Duration::from_secs(3);
        let before = terminal.received;
        while Instant::now() < deadline {
            terminal.read(false);
        }
        let sent = terminal.received - before;
        assert!(
            sent < MAX_UNACKED + 2 * BATCH,
            "sent {sent} bytes unacknowledged"
        );

        terminal
            .handle
            .ack(1, u32::try_from(terminal.received).unwrap());
        let resets = terminal.resets.len();
        terminal.wait_for("flood-42");
        assert!(terminal.resets.len() > resets, "catching up starts over");
        terminal.handle.stop(false).unwrap().join().unwrap();
    }
}
