//! `@path` in a prompt, as in Claude Code: the composer offers the folder's files, and a
//! prompt that mentions one sends its contents along, so the agent doesn't have to read it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The most paths [`list`] returns.
const MAX_LISTED: usize = 50;
/// A mentioned file bigger than this goes by name only, and the agent can read it itself.
const MAX_ATTACHED_BYTES: u64 = 256 * 1024;

/// The folder's files whose path matches `query`, best first. A git repo lists what git
/// would, so ignored files stay out.
pub async fn list(folder: &Path, query: &str) -> Result<Vec<String>> {
    let files = files_in(folder).await?;
    Ok(best_matches(&files, query))
}

async fn files_in(folder: &Path) -> Result<Vec<String>> {
    let output = tokio::process::Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(folder)
        .output()
        .await
        .context("couldn't run git")?;
    if output.status.success() {
        return Ok(output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect());
    }
    // Not a repo: walk it, keeping clear of what's usually huge and never wanted.
    let folder = folder.to_owned();
    tokio::task::spawn_blocking(move || walk(&folder))
        .await
        .context("listing the folder failed")
}

fn walk(folder: &Path) -> Vec<String> {
    const SKIPPED: &[&str] = &["node_modules", "target", ".git", "dist", "build"];
    let mut files = vec![];
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(folder.join(&relative)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = relative.join(&name);
            match entry.file_type() {
                Ok(kind) if kind.is_dir() && !SKIPPED.contains(&name.as_str()) => {
                    pending.push(path);
                }
                Ok(kind) if kind.is_file() => files.push(path.to_string_lossy().into_owned()),
                _ => {}
            }
            if files.len() > 20_000 {
                return files;
            }
        }
    }
    files
}

/// Paths containing `query`'s letters in order, scored so that a match in the file name,
/// at the start of a word, or in one piece ranks first. An empty query lists the shortest.
fn best_matches(files: &[String], query: &str) -> Vec<String> {
    let query = query.to_lowercase();
    let mut scored: Vec<(i64, &String)> = files
        .iter()
        .filter_map(|path| score(path, &query).map(|score| (score, path)))
        .collect();
    scored.sort_by(|(a, pa), (b, pb)| b.cmp(a).then(pa.len().cmp(&pb.len())).then(pa.cmp(pb)));
    scored
        .into_iter()
        .take(MAX_LISTED)
        .map(|(_, path)| path.clone())
        .collect()
}

fn score(path: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(-(path.len() as i64));
    }
    let lower = path.to_lowercase();
    let name_start = lower.rfind('/').map_or(0, |at| at + 1);
    if let Some(at) = lower.find(query) {
        let in_name = at >= name_start;
        return Some(1000 + if in_name { 500 } else { 0 } - at as i64 - lower.len() as i64);
    }
    let chars: Vec<char> = lower.chars().collect();
    let mut score = 0;
    let mut position = 0;
    for wanted in query.chars() {
        let found = chars[position..].iter().position(|c| *c == wanted)?;
        let at = position + found;
        if at == 0 || matches!(chars[at - 1], '/' | '_' | '-' | '.') {
            score += 10;
        }
        if found == 0 {
            score += 5;
        }
        position = at + 1;
    }
    Some(score - chars.len() as i64)
}

/// The files a prompt mentions as `@path`, relative to `folder`, with their text. A mention
/// that isn't a readable text file in the folder, or is too big, is left for the agent.
pub async fn attachments(folder: &Path, text: &str) -> Vec<(PathBuf, String)> {
    let mut found: Vec<(PathBuf, String)> = vec![];
    for mention in mentions(text) {
        let Some(path) = inside(folder, &mention) else {
            continue;
        };
        if found.iter().any(|(seen, _)| seen == &path) {
            continue;
        }
        let Ok(meta) = tokio::fs::metadata(&path).await else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_ATTACHED_BYTES {
            continue;
        }
        if let Ok(contents) = tokio::fs::read_to_string(&path).await {
            found.push((path, contents));
        }
    }
    found
}

/// What follows each `@` that starts a word, up to whitespace, without trailing punctuation.
fn mentions(text: &str) -> Vec<String> {
    let mut mentions = vec![];
    let mut previous = ' ';
    for (at, c) in text.char_indices() {
        if c == '@' && (previous.is_whitespace() || previous == '(') {
            let rest = &text[at + 1..];
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let mention = rest[..end].trim_end_matches(['.', ',', ';', ':', ')', '!', '?']);
            if !mention.is_empty() {
                mentions.push(mention.to_owned());
            }
        }
        previous = c;
    }
    mentions
}

/// The mentioned path, if it's inside the folder. A mention can't reach outside it.
fn inside(folder: &Path, mention: &str) -> Option<PathBuf> {
    let path = folder.join(mention);
    let real = path.canonicalize().ok()?;
    let root = folder.canonicalize().ok()?;
    real.starts_with(&root).then_some(real)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_rank_above_folders_and_gaps() {
        let files: Vec<String> = [
            "src/model.ts",
            "src/models/index.ts",
            "docs/my-other-design.md",
            "apps/dsk/src/AgentPane.tsx",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(best_matches(&files, "model")[0], "src/model.ts");
        assert_eq!(best_matches(&files, "agpa"), ["apps/dsk/src/AgentPane.tsx"]);
        assert!(best_matches(&files, "zzz").is_empty());
        assert_eq!(best_matches(&files, "").len(), 4);
    }

    #[test]
    fn mentions_start_words_and_lose_trailing_punctuation() {
        assert_eq!(
            mentions("look at @src/a.rs, and (@b.md). mail me@x.com"),
            ["src/a.rs", "b.md"]
        );
    }

    #[tokio::test]
    async fn mentioned_files_inside_the_folder_are_attached() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "no").unwrap();
        let escape = format!(
            "../{}/secret",
            outside.path().file_name().unwrap().to_string_lossy()
        );
        let text = format!("read @a.txt and @a.txt and @missing and @{escape}");
        let found = attachments(dir.path(), &text).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, "hello");
    }

    #[tokio::test]
    async fn a_folder_that_isnt_a_repo_is_walked() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/x")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
        std::fs::write(dir.path().join("node_modules/x/index.js"), "").unwrap();
        assert_eq!(list(dir.path(), "").await.unwrap(), ["src/main.rs"]);
    }
}
