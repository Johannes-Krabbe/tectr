//! Conversations Claude Code and Codex keep themselves, found on this machine and turned into
//! tectr's events, so a tab can show one and carry it on. Neither format is documented, so
//! anything not understood is left out rather than failing the import.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use protocol::{
    Conversation, SessionEventKind, StopReason, ToolCallContent, ToolCallPatch, ToolCallStatus,
};
use serde_json::{Value, json};

use crate::mcp::ConfigHome;

/// How much listing reads from each end of a file at most, so a long conversation doesn't slow
/// it down. What it needs is near the start, and Claude Code adds newer titles as it goes.
const HEAD_BYTES: u64 = 1 << 20;
const TAIL_BYTES: u64 = 64 << 10;
const TITLE_CHARS: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Claude,
    Codex,
}

impl Source {
    pub const ALL: [Source; 2] = [Source::Claude, Source::Codex];

    /// The agent that carries its conversations on.
    pub fn agent(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
        }
    }

    pub fn of_agent(agent: &str) -> Option<Source> {
        Source::ALL
            .into_iter()
            .find(|source| source.agent() == agent)
    }
}

/// A conversation as it was, ready to become a tab.
#[derive(Debug)]
pub struct Imported {
    pub title: String,
    pub cwd: String,
    /// In order, each with when it happened in milliseconds since the Unix epoch.
    pub events: Vec<(i64, SessionEventKind)>,
}

/// Every conversation the sources keep, newest first.
pub fn list(home: &ConfigHome, sources: &[Source]) -> Vec<Conversation> {
    let mut found = vec![];
    for source in sources {
        match source {
            Source::Claude => found.extend(claude::list(home)),
            Source::Codex => found.extend(codex::list(home)),
        }
    }
    found.sort_by_key(|conversation| std::cmp::Reverse(conversation.updated_at_ms));
    found
}

pub fn load(home: &ConfigHome, source: Source, id: &str) -> Result<Imported> {
    // IDs end up in file names, so nothing that could climb out of a folder gets that far.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("{id:?} isn't a conversation ID");
    }
    match source {
        Source::Claude => claude::load(home, id),
        Source::Codex => codex::load(home, id),
    }
}

/// Builds the events, keeping turns closed and separate messages apart. The app joins
/// consecutive messages into one, as it would streamed chunks.
#[derive(Default)]
struct Transcript {
    events: Vec<(i64, SessionEventKind)>,
    in_turn: bool,
    first_prompt: Option<String>,
    /// The agent's ID for the last thing it said this turn, as its adapter would report it.
    last_message: Option<String>,
}

impl Transcript {
    fn prompt(&mut self, at: i64, text: String) {
        if text.trim().is_empty() {
            return;
        }
        self.first_prompt.get_or_insert_with(|| text.clone());
        self.events.push((
            at,
            SessionEventKind::UserPrompt {
                text,
                images: vec![],
            },
        ));
        self.in_turn = true;
        self.last_message = None;
    }

    fn message(&mut self, at: i64, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let text = match self.events.last() {
            Some((_, SessionEventKind::AgentMessage { .. })) => format!("\n\n{text}"),
            _ => text.to_owned(),
        };
        self.events
            .push((at, SessionEventKind::AgentMessage { text }));
    }

    fn thought(&mut self, at: i64, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let text = match self.events.last() {
            Some((_, SessionEventKind::AgentThought { .. })) => format!("\n\n{text}"),
            _ => text.to_owned(),
        };
        self.events
            .push((at, SessionEventKind::AgentThought { text }));
    }

    fn tool(&mut self, at: i64, id: &str, tool: ToolCallPatch) {
        let id = id.to_owned();
        self.events
            .push((at, SessionEventKind::ToolCall { id, tool }));
    }

    fn tool_done(&mut self, at: i64, id: &str, status: ToolCallStatus) {
        let tool = ToolCallPatch {
            status: Some(status),
            ..ToolCallPatch::default()
        };
        let id = id.to_owned();
        self.events
            .push((at, SessionEventKind::ToolCallUpdate { id, tool }));
    }

    fn error(&mut self, at: i64, message: String) {
        self.events.push((at, SessionEventKind::Error { message }));
    }

    fn end_turn(&mut self, at: i64, stop_reason: StopReason) {
        let last_message = self.last_message.take();
        if std::mem::take(&mut self.in_turn) {
            self.events.push((
                at,
                SessionEventKind::TurnEnded {
                    stop_reason,
                    last_message,
                },
            ));
        }
    }

    fn finish(mut self, at: i64, title: Option<String>, cwd: Option<String>) -> Result<Imported> {
        self.end_turn(at, StopReason::EndTurn);
        let Some(first_prompt) = self.first_prompt else {
            bail!("the conversation has no messages");
        };
        Ok(Imported {
            title: title.unwrap_or_else(|| short(&first_prompt)),
            cwd: cwd.context("the conversation doesn't say which folder it was in")?,
            events: self.events,
        })
    }
}

/// The first line of a text, cut to fit a title.
fn short(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty());
    let line = line.unwrap_or_default();
    if line.chars().count() <= TITLE_CHARS {
        return line.to_owned();
    }
    let cut: String = line.chars().take(TITLE_CHARS).collect();
    format!("{}...", cut.trim_end())
}

fn str_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn timestamp(value: &Value) -> Option<i64> {
    let timestamp: jiff::Timestamp = str_at(value, "timestamp")?.parse().ok()?;
    Some(timestamp.as_millisecond())
}

/// A path as the person reads it: inside the conversation's folder, from there.
fn shown_path(path: &str, cwd: Option<&str>) -> String {
    let path = path.strip_prefix("file://").unwrap_or(path);
    cwd.and_then(|cwd| path.strip_prefix(cwd.trim_end_matches('/')))
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(path)
        .to_owned()
}

fn modified_ms(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn json_lines(reader: impl BufRead) -> impl Iterator<Item = Value> {
    // A line cut off by a crash, or by reading only part of a file, is skipped.
    reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
}

/// Reads files on every core, because listing reads hundreds of them.
fn summarize_all<T: Send>(
    paths: &[PathBuf],
    summarize: impl Fn(&Path) -> Option<T> + Sync,
) -> Vec<T> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = paths.len().div_ceil(threads).max(1);
    let summarize = &summarize;
    std::thread::scope(|scope| {
        let workers: Vec<_> = paths
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || chunk.iter().filter_map(|path| summarize(path)).collect())
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap_or_else(|_| Vec::new()))
            .collect()
    })
}

/// The whole lines in the last part of a file.
fn tail(mut file: File) -> Result<Vec<Value>> {
    let len = file.metadata()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut tail = BufReader::new(file);
    if start > 0 {
        // Starts partway through a line.
        tail.read_line(&mut String::new())?;
    }
    Ok(json_lines(tail).collect())
}

mod claude {
    use super::*;

    fn projects(home: &ConfigHome) -> Option<PathBuf> {
        home.claude_dir().map(|dir| dir.join("projects"))
    }

    /// Each folder Claude Code has worked in has a folder here, with a file per conversation.
    fn files(home: &ConfigHome) -> Vec<PathBuf> {
        let Some(Ok(folders)) = projects(home).map(std::fs::read_dir) else {
            return vec![];
        };
        folders
            .flatten()
            .filter_map(|folder| std::fs::read_dir(folder.path()).ok())
            .flat_map(|files| files.flatten().map(|file| file.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl") && path.is_file())
            .collect()
    }

    pub fn list(home: &ConfigHome) -> Vec<Conversation> {
        summarize_all(&files(home), |path| summarize(path).ok().flatten())
    }

    fn summarize(path: &Path) -> Result<Option<Conversation>> {
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .context("not a conversation file")?;
        let mut file = File::open(path)?;
        let mut titles = Titles::default();
        let mut cwd = None;
        let mut prompt = None;
        for entry in json_lines(BufReader::new((&mut file).take(HEAD_BYTES))) {
            titles.note(&entry);
            if cwd.is_none() {
                cwd = str_at(&entry, "cwd").map(str::to_owned);
            }
            if prompt.is_none() && !is_sidechain(&entry) {
                prompt = user_prompt(&entry);
            }
            if cwd.is_some() && prompt.is_some() && titles.best().is_some() {
                break;
            }
        }
        // Claude Code adds newer titles as it goes, and one someone gives it.
        tail(file)?.iter().for_each(|entry| titles.note(entry));
        // Without a prompt it's empty, like the ones left when tectr asks what Claude offers.
        let (Some(prompt), Some(cwd)) = (prompt, cwd) else {
            return Ok(None);
        };
        Ok(Some(Conversation {
            agent: Source::Claude.agent().to_owned(),
            id: id.to_owned(),
            title: titles.best().unwrap_or_else(|| short(&prompt)),
            cwd,
            updated_at_ms: modified_ms(path),
        }))
    }

    pub fn load(home: &ConfigHome, id: &str) -> Result<Imported> {
        let name = format!("{id}.jsonl");
        let path = files(home)
            .into_iter()
            .find(|path| path.file_name().is_some_and(|file| file == name.as_str()))
            .with_context(|| format!("Claude Code has no conversation {id} on this device"))?;
        let reader = BufReader::new(File::open(&path)?);
        // Requests to the API are the bulk of a file and play no part here.
        let entries: Vec<Value> = json_lines(reader)
            .filter(|entry| entry.get("uuid").is_some() || Titles::is_title(entry))
            .collect();
        convert(&entries)
    }

    pub(super) fn convert(entries: &[Value]) -> Result<Imported> {
        let branch = active_branch(entries);
        let mut transcript = Transcript::default();
        let mut titles = Titles::default();
        let mut cwd: Option<String> = None;
        let mut at = 0;
        for entry in entries {
            titles.note(entry);
            at = timestamp(entry).unwrap_or(at);
            if cwd.is_none() {
                cwd = str_at(entry, "cwd").map(str::to_owned);
            }
            let on_branch = str_at(entry, "uuid").is_some_and(|uuid| branch.contains(uuid));
            if !on_branch || is_sidechain(entry) {
                continue;
            }
            match str_at(entry, "type") {
                Some("user") => user(&mut transcript, at, entry),
                Some("assistant") => assistant(&mut transcript, at, entry, cwd.as_deref()),
                _ => {}
            }
        }
        transcript.finish(at, titles.best(), cwd)
    }

    /// Going back from the latest message leaves out what was rewound and tried again.
    fn active_branch(entries: &[Value]) -> HashSet<&str> {
        let by_id: HashMap<&str, &Value> = entries
            .iter()
            .filter_map(|entry| Some((str_at(entry, "uuid")?, entry)))
            .collect();
        let leaf = entries.iter().rev().find(|entry| {
            matches!(str_at(entry, "type"), Some("user" | "assistant")) && !is_sidechain(entry)
        });
        let mut branch = HashSet::new();
        let mut next = leaf.and_then(|entry| str_at(entry, "uuid"));
        while let Some(id) = next {
            if !branch.insert(id) {
                break;
            }
            // Compacting starts over with no parent, and names the one it would have had.
            next = by_id.get(id).and_then(|entry| {
                str_at(entry, "parentUuid").or_else(|| str_at(entry, "logicalParentUuid"))
            });
        }
        branch
    }

    fn is_sidechain(entry: &Value) -> bool {
        entry.get("isSidechain").and_then(Value::as_bool) == Some(true)
    }

    fn flag(entry: &Value, key: &str) -> bool {
        entry.get(key).and_then(Value::as_bool) == Some(true)
    }

    fn blocks(entry: &Value) -> Vec<Value> {
        match entry.pointer("/message/content") {
            Some(Value::String(text)) => vec![json!({ "type": "text", "text": text })],
            Some(Value::Array(blocks)) => blocks.clone(),
            _ => vec![],
        }
    }

    /// What a person typed, for a user entry that has some. Claude Code also stores its own
    /// additions as user messages, wrapped in tags.
    fn user_prompt(entry: &Value) -> Option<String> {
        if str_at(entry, "type") != Some("user")
            || flag(entry, "isMeta")
            || flag(entry, "isCompactSummary")
            || flag(entry, "isVisibleInTranscriptOnly")
        {
            return None;
        }
        let parts: Vec<String> = blocks(entry)
            .iter()
            .filter_map(|block| match str_at(block, "type") {
                Some("text") => typed_text(str_at(block, "text")?),
                Some("image") => Some("[Image]".to_owned()),
                _ => None,
            })
            .collect();
        let text = parts.join("\n\n");
        (!text.trim().is_empty()).then_some(text)
    }

    fn typed_text(text: &str) -> Option<String> {
        let text = text.trim();
        if let Some(name) = between(text, "<command-name>", "</command-name>") {
            let name = name.trim_start_matches('/');
            let args = between(text, "<command-args>", "</command-args>").unwrap_or_default();
            return Some(format!("/{name} {args}").trim_end().to_owned());
        }
        let tag = text
            .strip_prefix('<')
            .and_then(|rest| rest.split_once('>'))
            .map(|(tag, _)| tag)
            .filter(|tag| {
                !tag.is_empty() && tag.chars().all(|c| c.is_ascii_lowercase() || c == '-')
            });
        if tag.is_some_and(|tag| text.ends_with(&format!("</{tag}>"))) {
            return None;
        }
        Some(text.to_owned())
    }

    fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
        let rest = &text[text.find(start)? + start.len()..];
        Some(rest[..rest.find(end)?].trim())
    }

    fn user(transcript: &mut Transcript, at: i64, entry: &Value) {
        for block in blocks(entry) {
            if str_at(&block, "type") == Some("tool_result")
                && let Some(id) = str_at(&block, "tool_use_id")
            {
                let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
                let status = if failed {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Completed
                };
                transcript.tool_done(at, id, status);
            }
        }
        let Some(text) = user_prompt(entry) else {
            return;
        };
        if text.starts_with("[Request interrupted by user") {
            transcript.end_turn(at, StopReason::Cancelled);
            return;
        }
        transcript.end_turn(at, StopReason::EndTurn);
        transcript.prompt(at, text);
    }

    fn assistant(transcript: &mut Transcript, at: i64, entry: &Value, cwd: Option<&str>) {
        let blocks = blocks(entry);
        if flag(entry, "isApiErrorMessage") {
            let text: Vec<&str> = blocks.iter().filter_map(|b| str_at(b, "text")).collect();
            transcript.error(at, text.join("\n"));
            return;
        }
        // What Claude's adapter calls a message, so a branch can start after it.
        let id = entry.pointer("/message/id").and_then(Value::as_str);
        if let Some(id) = id.or_else(|| str_at(entry, "uuid")) {
            transcript.last_message = Some(id.to_owned());
        }
        for block in &blocks {
            match str_at(block, "type") {
                Some("text") => transcript.message(at, str_at(block, "text").unwrap_or_default()),
                Some("thinking") => {
                    transcript.thought(at, str_at(block, "thinking").unwrap_or_default());
                }
                Some("tool_use") => {
                    let (Some(id), Some(name)) = (str_at(block, "id"), str_at(block, "name"))
                    else {
                        continue;
                    };
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    transcript.tool(at, id, tool(name, input, cwd));
                }
                _ => {}
            }
        }
    }

    /// Close to what the Claude adapter shows for the same call.
    fn tool(name: &str, input: Value, cwd: Option<&str>) -> ToolCallPatch {
        let field = |key: &str| str_at(&input, key).map(str::to_owned);
        let path = field("file_path").or_else(|| field("notebook_path"));
        let shown = path.as_deref().map(|path| shown_path(path, cwd));
        let on = |verb: &str| match &shown {
            Some(path) => format!("{verb} {path}"),
            None => verb.to_owned(),
        };
        let (title, kind) = match name {
            "Bash" => (
                field("command").unwrap_or_else(|| "Terminal".to_owned()),
                "execute",
            ),
            "Read" => (on("Read"), "read"),
            "Edit" | "MultiEdit" | "NotebookEdit" => (on("Edit"), "edit"),
            "Write" => (on("Write"), "edit"),
            "Grep" => (
                format!("grep {}", field("pattern").unwrap_or_default()),
                "search",
            ),
            "Glob" => (
                format!("Find {}", field("pattern").unwrap_or_default()),
                "search",
            ),
            "WebFetch" => (
                format!("Fetch {}", field("url").unwrap_or_default()),
                "fetch",
            ),
            "WebSearch" => (
                format!("Search for {}", field("query").unwrap_or_default()),
                "fetch",
            ),
            "Task" | "Agent" => (
                field("description").unwrap_or_else(|| name.to_owned()),
                "think",
            ),
            "TodoWrite" => ("Update the plan".to_owned(), "think"),
            _ => (name.to_owned(), "other"),
        };
        let content = match (name, &path) {
            ("Edit", Some(path)) => field("new_string").map(|new_text| ToolCallContent::Diff {
                path: path.clone(),
                old_text: field("old_string"),
                new_text,
            }),
            ("Write", Some(path)) => field("content").map(|new_text| ToolCallContent::Diff {
                path: path.clone(),
                old_text: None,
                new_text,
            }),
            _ => None,
        };
        ToolCallPatch {
            title: Some(title.trim().to_owned()),
            kind: Some(kind.to_owned()),
            status: None,
            locations: path.map(|path| vec![path]),
            content: content.map(|content| vec![content]),
            input: Some(input),
        }
    }

    /// A title someone gave it wins over one Claude Code made up, and later ones win.
    #[derive(Default)]
    struct Titles {
        custom: Option<String>,
        generated: Option<String>,
        summary: Option<String>,
    }

    impl Titles {
        fn is_title(entry: &Value) -> bool {
            matches!(
                str_at(entry, "type"),
                Some("custom-title" | "ai-title" | "summary")
            )
        }

        fn note(&mut self, entry: &Value) {
            let (slot, key) = match str_at(entry, "type") {
                Some("custom-title") => (&mut self.custom, "customTitle"),
                Some("ai-title") => (&mut self.generated, "aiTitle"),
                Some("summary") => (&mut self.summary, "summary"),
                _ => return,
            };
            if let Some(title) = str_at(entry, key).map(str::trim).filter(|t| !t.is_empty()) {
                *slot = Some(title.to_owned());
            }
        }

        fn best(&self) -> Option<String> {
            self.custom
                .clone()
                .or_else(|| self.generated.clone())
                .or_else(|| self.summary.clone())
        }
    }
}

mod codex {
    use super::*;

    fn sessions(home: &ConfigHome) -> Option<PathBuf> {
        home.codex_dir().map(|dir| dir.join("sessions"))
    }

    /// Conversations are filed by day, as `YYYY/MM/DD/rollout-<time>-<id>.jsonl`, and older
    /// ones are compressed.
    fn files(home: &ConfigHome) -> Vec<PathBuf> {
        let mut found = vec![];
        if let Some(dir) = sessions(home) {
            walk(&dir, 3, &mut found);
        }
        found
    }

    fn walk(dir: &Path, depth: u32, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.is_dir() {
                if depth > 0 {
                    walk(&path, depth - 1, found);
                }
            } else if id_of(&path).is_some() {
                found.push(path);
            }
        }
    }

    fn id_of(path: &Path) -> Option<&str> {
        let name = path.file_name()?.to_str()?;
        let stem = name
            .strip_suffix(".jsonl.zst")
            .or_else(|| name.strip_suffix(".jsonl"))?;
        stem.strip_prefix("rollout-")?;
        // The time before it is made of digits and dashes too, and an ID is a UUID.
        let start = stem.len().checked_sub(36)?;
        stem.get(start..)
    }

    fn open(path: &Path) -> Result<Box<dyn BufRead>> {
        let file = File::open(path)?;
        if path.extension().is_some_and(|ext| ext == "zst") {
            let decoder = ruzstd::decoding::StreamingDecoder::new(file)
                .map_err(|error| anyhow::anyhow!("failed to decompress: {error}"))?;
            Ok(Box::new(BufReader::new(decoder)))
        } else {
            Ok(Box::new(BufReader::new(file)))
        }
    }

    /// The names Codex gave its conversations, newest last.
    fn names(home: &ConfigHome) -> HashMap<String, String> {
        let Some(Ok(file)) = home
            .codex_dir()
            .map(|dir| File::open(dir.join("session_index.jsonl")))
        else {
            return HashMap::new();
        };
        json_lines(BufReader::new(file))
            .filter_map(|line| {
                let id = str_at(&line, "id")?.to_owned();
                let name = str_at(&line, "thread_name")?.trim().to_owned();
                (!name.is_empty()).then_some((id, name))
            })
            .collect()
    }

    pub fn list(home: &ConfigHome) -> Vec<Conversation> {
        let names = names(home);
        let mut newest: HashMap<String, Conversation> = HashMap::new();
        let found = summarize_all(&files(home), |path| summarize(path, &names).ok().flatten());
        for conversation in found {
            let newer = newest
                .get(&conversation.id)
                .is_none_or(|known| known.updated_at_ms < conversation.updated_at_ms);
            if newer {
                newest.insert(conversation.id.clone(), conversation);
            }
        }
        newest.into_values().collect()
    }

    fn summarize(path: &Path, names: &HashMap<String, String>) -> Result<Option<Conversation>> {
        let mut lines = json_lines(open(path)?.take(HEAD_BYTES));
        let meta = lines.next().context("the conversation is empty")?;
        let payload = meta.get("payload").context("no session details")?;
        // Agents an agent started have conversations of their own, which nobody typed in.
        if str_at(payload, "thread_source").is_some_and(|source| source != "user") {
            return Ok(None);
        }
        let (Some(id), Some(cwd)) = (str_at(payload, "id"), str_at(payload, "cwd")) else {
            return Ok(None);
        };
        let title = names.get(id).cloned().or_else(|| {
            lines
                .find_map(|line| prompt_of(&line))
                .as_deref()
                .map(short)
        });
        // Without a name or a prompt, nobody has typed anything in it.
        let Some(title) = title else {
            return Ok(None);
        };
        Ok(Some(Conversation {
            agent: Source::Codex.agent().to_owned(),
            id: id.to_owned(),
            title,
            cwd: cwd.to_owned(),
            updated_at_ms: modified_ms(path),
        }))
    }

    /// What a person typed, from a line that has it in either the newer or the older format.
    fn prompt_of(line: &Value) -> Option<String> {
        let payload = line.get("payload")?;
        match str_at(payload, "type")? {
            "item_completed" => {
                let item = payload.get("item")?;
                (str_at(item, "type")? == "UserMessage").then(|| user_text(item))
            }
            "user_message" => str_at(payload, "message").map(str::to_owned),
            _ => None,
        }
        .filter(|text| !text.trim().is_empty())
    }

    pub fn load(home: &ConfigHome, id: &str) -> Result<Imported> {
        let path = files(home)
            .into_iter()
            .filter(|path| id_of(path) == Some(id))
            .max_by_key(|path| modified_ms(path))
            .with_context(|| format!("Codex has no conversation {id} on this device"))?;
        let lines: Vec<Value> = json_lines(open(&path)?).collect();
        convert(&lines, names(home).remove(id))
    }

    pub(super) fn convert(lines: &[Value], name: Option<String>) -> Result<Imported> {
        // Newer versions record each finished item, which is what their app shows. Older ones
        // only have their events, with less detail.
        let items = lines.iter().any(|line| {
            line.pointer("/payload/type").and_then(Value::as_str) == Some("item_completed")
        });
        let mut transcript = Transcript::default();
        let mut cwd: Option<String> = None;
        let mut at = 0;
        for line in lines {
            at = timestamp(line).unwrap_or(at);
            let Some(payload) = line.get("payload") else {
                continue;
            };
            let kind = str_at(payload, "type").unwrap_or_default();
            match (str_at(line, "type").unwrap_or_default(), kind) {
                ("session_meta", _) => {
                    cwd = cwd.or_else(|| str_at(payload, "cwd").map(str::to_owned));
                }
                ("event_msg", "item_completed") if items => {
                    if let Some(item) = payload.get("item") {
                        item_event(&mut transcript, at, item, cwd.as_deref());
                    }
                }
                ("event_msg", "user_message") if !items => {
                    let text = str_at(payload, "message").unwrap_or_default();
                    transcript.prompt(at, text.to_owned());
                }
                ("event_msg", "agent_message") if !items => {
                    transcript.message(at, str_at(payload, "message").unwrap_or_default());
                }
                ("event_msg", "agent_reasoning") if !items => {
                    transcript.thought(at, str_at(payload, "text").unwrap_or_default());
                }
                ("event_msg", "task_complete") => transcript.end_turn(at, StopReason::EndTurn),
                ("event_msg", "turn_aborted") => transcript.end_turn(at, StopReason::Cancelled),
                _ => {}
            }
        }
        transcript.finish(at, name, cwd)
    }

    fn texts(value: Option<&Value>, key: &str) -> Vec<String> {
        let Some(Value::Array(parts)) = value else {
            return vec![];
        };
        parts
            .iter()
            .filter_map(|part| match part {
                Value::String(text) => Some(text.clone()),
                part => str_at(part, key).map(str::to_owned),
            })
            .collect()
    }

    fn user_text(item: &Value) -> String {
        let Some(Value::Array(parts)) = item.get("content") else {
            return String::new();
        };
        let parts: Vec<String> = parts
            .iter()
            .filter_map(|part| match str_at(part, "type") {
                Some("text") => str_at(part, "text").map(str::to_owned),
                Some("image" | "local_image") => Some("[Image]".to_owned()),
                _ => None,
            })
            .collect();
        parts.join("\n\n")
    }

    fn status(item: &Value) -> ToolCallStatus {
        match str_at(item, "status") {
            Some("failed" | "declined") => ToolCallStatus::Failed,
            _ => ToolCallStatus::Completed,
        }
    }

    /// A command as the person would type it, without the shell Codex runs it in.
    fn command_text(command: Option<&Value>) -> String {
        match command {
            Some(Value::String(command)) => command.clone(),
            Some(Value::Array(parts)) => {
                let parts: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
                match parts.as_slice() {
                    [_, "-lc" | "-c", command] => (*command).to_owned(),
                    parts => parts.join(" "),
                }
            }
            _ => "Terminal".to_owned(),
        }
    }

    fn item_event(transcript: &mut Transcript, at: i64, item: &Value, cwd: Option<&str>) {
        let id = str_at(item, "id").unwrap_or_default();
        let tool = |title: String, kind: &str, input: Option<Value>, locations| ToolCallPatch {
            title: Some(title),
            kind: Some(kind.to_owned()),
            status: Some(status(item)),
            input,
            locations,
            content: None,
        };
        match str_at(item, "type").unwrap_or_default() {
            "UserMessage" => transcript.prompt(at, user_text(item)),
            "AgentMessage" => transcript.message(at, &texts(item.get("content"), "text").concat()),
            "Plan" => transcript.message(at, str_at(item, "text").unwrap_or_default()),
            "Reasoning" => {
                transcript.thought(at, &texts(item.get("summary_text"), "text").join("\n\n"));
            }
            "CommandExecution" => {
                let command = command_text(item.get("command"));
                let input = json!({ "command": command });
                transcript.tool(at, id, tool(command, "execute", Some(input), None));
            }
            "FileChange" => {
                let paths: Vec<String> = item
                    .get("changes")
                    .and_then(Value::as_object)
                    .map(|changes| changes.keys().cloned().collect())
                    .unwrap_or_default();
                let title = match paths.as_slice() {
                    [path] => format!("Edit {}", shown_path(path, cwd)),
                    paths => format!("Edit {} files", paths.len()),
                };
                transcript.tool(at, id, tool(title, "edit", None, Some(paths)));
            }
            "McpToolCall" => {
                let name = str_at(item, "tool").unwrap_or("Tool call").to_owned();
                let input = item.get("arguments").cloned();
                transcript.tool(at, id, tool(name, "other", input, None));
            }
            "WebSearch" => {
                let title = format!("Search for {}", str_at(item, "query").unwrap_or_default());
                transcript.tool(at, id, tool(title, "fetch", None, None));
            }
            "Extension" if str_at(item, "kind") == Some("web.search") => {
                let title = format!("Search for {}", str_at(item, "query").unwrap_or_default());
                transcript.tool(at, id, tool(title, "fetch", None, None));
            }
            "ImageView" => {
                let path = str_at(item, "path").unwrap_or_default();
                let title = format!("View {}", shown_path(path, cwd));
                transcript.tool(at, id, tool(title, "read", None, None));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<Value> {
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn kinds(imported: &Imported) -> Vec<&SessionEventKind> {
        imported.events.iter().map(|(_, kind)| kind).collect()
    }

    fn prompts(imported: &Imported) -> Vec<&str> {
        imported
            .events
            .iter()
            .filter_map(|(_, kind)| match kind {
                SessionEventKind::UserPrompt { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn home(dir: &Path) -> ConfigHome {
        ConfigHome {
            home: Some(dir.to_owned()),
            codex_home: None,
            xdg_config: None,
            opencode_config: None,
            opencode_content: None,
            claude_code: None,
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    const CLAUDE: &str = r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-11-10T11:30:26.683Z"}
{"parentUuid":null,"isSidechain":false,"type":"user","message":{"role":"user","content":[{"type":"text","text":"Fix the login bug"}]},"uuid":"u1","timestamp":"2026-11-10T11:30:26.729Z","cwd":"/repo"}
{"parentUuid":"u1","isSidechain":false,"type":"attachment","uuid":"a1","timestamp":"2026-11-10T11:30:26.730Z"}
{"type":"ai-title","aiTitle":"Fix the login bug"}
{"parentUuid":"a1","isSidechain":false,"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Look at auth first."}]},"uuid":"x1","timestamp":"2026-11-10T11:30:28.000Z"}
{"parentUuid":"x1","isSidechain":false,"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/repo/src/auth.rs"}}]},"uuid":"x2","timestamp":"2026-11-10T11:30:29.000Z"}
{"parentUuid":"x2","isSidechain":false,"type":"user","message":{"role":"user","content":[{"tool_use_id":"t1","type":"tool_result","content":"fn login() {}","is_error":false}]},"uuid":"r1","timestamp":"2026-11-10T11:30:30.000Z"}
{"parentUuid":"r1","isSidechain":true,"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"a subagent talking"}]},"uuid":"s1","timestamp":"2026-11-10T11:30:30.500Z"}
{"parentUuid":"r1","isSidechain":false,"type":"assistant","message":{"id":"msg_found","role":"assistant","content":[{"type":"text","text":"Found it."}]},"uuid":"x3","timestamp":"2026-11-10T11:30:31.000Z"}
{"parentUuid":"x3","isSidechain":false,"type":"user","message":{"role":"user","content":"Abandoned idea"},"uuid":"u2","timestamp":"2026-11-10T11:31:00.000Z"}
{"parentUuid":"x3","isSidechain":false,"type":"user","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>Caveat</local-command-caveat>"},"uuid":"m1","timestamp":"2026-11-10T11:31:10.000Z"}
{"parentUuid":"m1","isSidechain":false,"type":"user","message":{"role":"user","content":"<command-name>/review</command-name>\n<command-message>review</command-message>\n<command-args>the diff</command-args>"},"uuid":"u3","timestamp":"2026-11-10T11:31:20.000Z"}
{"parentUuid":"u3","isSidechain":false,"type":"user","message":{"role":"user","content":"<local-command-stdout>ok</local-command-stdout>"},"uuid":"o1","timestamp":"2026-11-10T11:31:21.000Z"}
{"parentUuid":"o1","isSidechain":false,"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Reviewing."}]},"uuid":"x4","timestamp":"2026-11-10T11:31:22.000Z"}
{"parentUuid":"x4","isSidechain":false,"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]},"uuid":"i1","timestamp":"2026-11-10T11:31:23.000Z"}
{"type":"custom-title","customTitle":"Login bug"}"#;

    #[test]
    fn a_claude_conversation_keeps_its_latest_branch_and_drops_what_claude_added() {
        let imported = claude::convert(&lines(CLAUDE)).unwrap();
        assert_eq!(imported.title, "Login bug");
        assert_eq!(imported.cwd, "/repo");
        // The rewound prompt is gone, and the slash command reads as it was typed.
        assert_eq!(
            prompts(&imported),
            ["Fix the login bug", "/review the diff"]
        );
        let read = ToolCallPatch {
            title: Some("Read src/auth.rs".into()),
            kind: Some("read".into()),
            status: None,
            input: Some(json!({ "file_path": "/repo/src/auth.rs" })),
            locations: Some(vec!["/repo/src/auth.rs".into()]),
            content: None,
        };
        let done = ToolCallPatch {
            status: Some(ToolCallStatus::Completed),
            ..ToolCallPatch::default()
        };
        assert_eq!(
            kinds(&imported),
            [
                &SessionEventKind::UserPrompt {
                    text: "Fix the login bug".into(),
                    images: vec![]
                },
                &SessionEventKind::AgentThought {
                    text: "Look at auth first.".into()
                },
                &SessionEventKind::ToolCall {
                    id: "t1".into(),
                    tool: read
                },
                &SessionEventKind::ToolCallUpdate {
                    id: "t1".into(),
                    tool: done
                },
                &SessionEventKind::AgentMessage {
                    text: "Found it.".into()
                },
                // The API's ID where there is one, as the adapter reports it.
                &SessionEventKind::TurnEnded {
                    stop_reason: StopReason::EndTurn,
                    last_message: Some("msg_found".into()),
                },
                &SessionEventKind::UserPrompt {
                    text: "/review the diff".into(),
                    images: vec![]
                },
                &SessionEventKind::AgentMessage {
                    text: "Reviewing.".into()
                },
                &SessionEventKind::TurnEnded {
                    stop_reason: StopReason::Cancelled,
                    last_message: Some("x4".into()),
                },
            ]
        );
        // Each event keeps when it happened.
        assert_eq!(imported.events[0].0, 1_794_310_226_729);
    }

    const CODEX: &str = r##"{"timestamp":"2026-10-29T16:21:30.499Z","type":"session_meta","payload":{"id":"01a18876-b39d-75f2-906c-f7a10fd228b2","cwd":"/repo","thread_source":"user"}}
{"timestamp":"2026-10-29T16:21:30.501Z","type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}
{"timestamp":"2026-10-29T16:21:30.858Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions"}]}}
{"timestamp":"2026-10-29T16:21:30.899Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"m1","content":[{"type":"text","text":"check the diff","text_elements":[]}]}}}
{"timestamp":"2026-10-29T16:21:31.000Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"Reasoning","id":"r1","summary_text":[],"raw_content":[]}}}
{"timestamp":"2026-10-29T16:21:33.626Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","id":"a1","content":[{"type":"Text","text":"Reading the diff."}],"phase":"commentary"}}}
{"timestamp":"2026-10-29T16:21:35.000Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"c1","command":["/bin/zsh","-lc","git diff"],"status":"failed"}}}
{"timestamp":"2026-10-29T16:21:36.000Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"FileChange","id":"f1","changes":{"/repo/a.rs":{"type":"add","content":"x"}}}}}
{"timestamp":"2026-10-29T16:21:37.000Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"AgentMessage","id":"a2","content":[{"type":"Text","text":"Looks fine."}],"phase":"final"}}}
{"timestamp":"2026-10-29T16:21:38.000Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1"}}
{"timestamp":"2026-10-29T16:22:00.000Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"m2","content":[{"type":"text","text":"and now?"}]}}}
{"timestamp":"2026-10-29T16:22:01.000Z","type":"event_msg","payload":{"type":"turn_aborted","reason":"interrupted"}}"##;

    #[test]
    fn a_codex_conversation_becomes_prompts_messages_and_tool_calls() {
        let imported = codex::convert(&lines(CODEX), None).unwrap();
        assert_eq!(imported.title, "check the diff");
        assert_eq!(imported.cwd, "/repo");
        let tool = |title: &str, kind: &str, status, input, locations| ToolCallPatch {
            title: Some(title.into()),
            kind: Some(kind.into()),
            status: Some(status),
            input,
            locations,
            content: None,
        };
        assert_eq!(
            kinds(&imported),
            [
                &SessionEventKind::UserPrompt {
                    text: "check the diff".into(),
                    images: vec![]
                },
                &SessionEventKind::AgentMessage {
                    text: "Reading the diff.".into()
                },
                &SessionEventKind::ToolCall {
                    id: "c1".into(),
                    tool: tool(
                        "git diff",
                        "execute",
                        ToolCallStatus::Failed,
                        Some(json!({ "command": "git diff" })),
                        None
                    ),
                },
                &SessionEventKind::ToolCall {
                    id: "f1".into(),
                    tool: tool(
                        "Edit a.rs",
                        "edit",
                        ToolCallStatus::Completed,
                        None,
                        Some(vec!["/repo/a.rs".into()])
                    ),
                },
                &SessionEventKind::AgentMessage {
                    text: "Looks fine.".into()
                },
                &SessionEventKind::TurnEnded {
                    stop_reason: StopReason::EndTurn,
                    last_message: None,
                },
                &SessionEventKind::UserPrompt {
                    text: "and now?".into(),
                    images: vec![]
                },
                &SessionEventKind::TurnEnded {
                    stop_reason: StopReason::Cancelled,
                    last_message: None,
                },
            ]
        );
    }

    #[test]
    fn older_codex_conversations_without_items_still_import() {
        let old = r#"{"timestamp":"2025-06-01T10:00:00.000Z","type":"session_meta","payload":{"id":"x","cwd":"/repo"}}
{"timestamp":"2025-06-01T10:00:01.000Z","type":"event_msg","payload":{"type":"user_message","message":"hello"}}
{"timestamp":"2025-06-01T10:00:02.000Z","type":"event_msg","payload":{"type":"agent_message","message":"hi"}}
{"timestamp":"2025-06-01T10:00:02.000Z","type":"event_msg","payload":{"type":"agent_message","message":"there"}}"#;
        let imported = codex::convert(&lines(old), Some("Greeting".into())).unwrap();
        assert_eq!(imported.title, "Greeting");
        assert_eq!(
            kinds(&imported),
            [
                &SessionEventKind::UserPrompt {
                    text: "hello".into(),
                    images: vec![]
                },
                &SessionEventKind::AgentMessage { text: "hi".into() },
                // Kept apart, because the app joins consecutive messages.
                &SessionEventKind::AgentMessage {
                    text: "\n\nthere".into()
                },
                &SessionEventKind::TurnEnded {
                    stop_reason: StopReason::EndTurn,
                    last_message: None,
                },
            ]
        );
    }

    #[test]
    fn listing_finds_both_agents_conversations_newest_first_and_skips_empty_ones() {
        let dir = tempfile::tempdir().unwrap();
        let home = home(dir.path());
        let projects = dir.path().join(".claude/projects/-repo");
        write(&projects.join("aaaa-1111.jsonl"), CLAUDE);
        let older = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        File::options()
            .write(true)
            .open(projects.join("aaaa-1111.jsonl"))
            .unwrap()
            .set_modified(older)
            .unwrap();
        // What asking Claude what it offers can leave behind.
        write(
            &projects.join("bbbb-2222.jsonl"),
            r#"{"type":"queue-operation","operation":"enqueue"}"#,
        );
        let id = "01a18876-b39d-75f2-906c-f7a10fd228b2";
        let day = dir.path().join(".codex/sessions/2026/10/29");
        write(
            &day.join(format!("rollout-2026-10-29T17-20-43-{id}.jsonl")),
            CODEX,
        );
        write(
            &day.join("rollout-2026-10-29T17-20-44-01a18876-0000-75f2-906c-f7a10fd228b2.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"01a18876-0000-75f2-906c-f7a10fd228b2","cwd":"/repo","thread_source":"subagent"}}"#,
        );
        write(
            &dir.path().join(".codex/session_index.jsonl"),
            &format!(
                "{{\"id\":\"{id}\",\"thread_name\":\"check\"}}\n{{\"id\":\"{id}\",\"thread_name\":\"Check the diff\"}}\n"
            ),
        );

        let found = list(&home, &Source::ALL);
        let summary: Vec<(&str, &str, &str)> = found
            .iter()
            .map(|c| (c.agent.as_str(), c.id.as_str(), c.title.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                ("codex", id, "Check the diff"),
                ("claude", "aaaa-1111", "Login bug")
            ]
        );
        assert!(found.iter().all(|c| c.cwd == "/repo"));
        assert_eq!(list(&home, &[Source::Claude]).len(), 1);

        let imported = load(&home, Source::Codex, id).unwrap();
        assert_eq!(imported.title, "Check the diff");
        assert!(load(&home, Source::Claude, "aaaa-1111").is_ok());
        assert!(load(&home, Source::Claude, "../aaaa-1111").is_err());
        assert!(load(&home, Source::Claude, "missing").is_err());
    }

    #[test]
    fn compressed_codex_conversations_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let id = "01a18876-b39d-75f2-906c-f7a10fd228b2";
        let path = dir.path().join(format!(
            ".codex/sessions/2026/10/29/rollout-2026-10-29T17-20-43-{id}.jsonl.zst"
        ));
        let compressed = ruzstd::encoding::compress_to_vec(
            CODEX.as_bytes(),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, compressed).unwrap();

        let home = home(dir.path());
        assert_eq!(list(&home, &[Source::Codex])[0].title, "check the diff");
        assert_eq!(
            prompts(&load(&home, Source::Codex, id).unwrap()),
            ["check the diff", "and now?"]
        );
    }
}
