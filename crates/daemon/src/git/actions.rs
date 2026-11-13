//! What a diff tab can do to a folder: stage, unstage, discard and commit. Hunks go through
//! `git apply` with a patch built from the hunk as the client saw it, after checking the file
//! still has exactly that hunk.

use std::path::{Component, Path};
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use protocol::{FileBody, Hunk};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::diff::{diff_args, parse, run};

/// Which way a hunk goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkAction {
    Stage,
    Unstage,
    Discard,
}

pub async fn stage_files(dir: &Path, paths: &[String]) -> Result<()> {
    check_paths(paths)?;
    // `-A` also stages a deletion.
    let mut args = vec!["add", "-A", "--"];
    args.extend(paths.iter().map(String::as_str));
    run(dir, &args).await?;
    Ok(())
}

pub async fn unstage_files(dir: &Path, paths: &[String]) -> Result<()> {
    check_paths(paths)?;
    // With no commit yet there's nothing to restore from, so the index entry just goes.
    let mut args = if has_head(dir).await {
        vec!["restore", "--staged", "--"]
    } else {
        vec!["rm", "--cached", "-q", "-r", "--"]
    };
    args.extend(paths.iter().map(String::as_str));
    run(dir, &args).await?;
    Ok(())
}

/// Puts files back as they're staged, or as committed when nothing is. Untracked files are
/// deleted.
pub async fn discard_files(dir: &Path, paths: &[String]) -> Result<()> {
    check_paths(paths)?;
    let mut tracked = vec![];
    for path in paths {
        let known = run(dir, &["ls-files", "--", path]).await?;
        if known.trim().is_empty() {
            let full = dir.join(path);
            match tokio::fs::remove_file(&full).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| format!("failed to delete {path}"));
                }
            }
        } else {
            tracked.push(path.as_str());
        }
    }
    if !tracked.is_empty() {
        let mut args = vec!["restore", "--worktree", "--"];
        args.extend(tracked);
        run(dir, &args).await?;
    }
    Ok(())
}

pub async fn apply_hunk(dir: &Path, path: &str, hunk: &Hunk, action: HunkAction) -> Result<()> {
    check_paths(std::slice::from_ref(&path.to_owned()))?;
    // Staging takes the hunk from what isn't staged yet, unstaging from what is.
    let staged = action == HunkAction::Unstage;
    let mut args = diff_args(if staged { &["--cached"] } else { &[] });
    args.extend(["--", path]);
    let files = parse(&run(dir, &args).await?, staged);
    let current = files
        .iter()
        .find(|file| file.path == path)
        .and_then(|file| match &file.body {
            FileBody::Text { hunks } => hunks.iter().find(|h| *h == hunk),
            _ => None,
        });
    if current.is_none() {
        bail!("{path} changed since it was shown. Try again once it's caught up");
    }
    let patch = patch_for(path, hunk);
    let mut args = vec!["apply", "--whitespace=nowarn"];
    match action {
        HunkAction::Stage => args.push("--cached"),
        HunkAction::Unstage => args.extend(["--cached", "--reverse"]),
        HunkAction::Discard => args.push("--reverse"),
    }
    args.push("-");
    with_input(dir, &args, &patch).await
}

/// Commits what's staged. The message goes in on stdin, so any text works.
pub async fn commit(dir: &Path, message: &str, amend: bool) -> Result<()> {
    let message = message.trim();
    if message.is_empty() && !amend {
        bail!("a commit needs a message");
    }
    let mut args = vec!["commit", "-q"];
    if amend {
        args.push("--amend");
    }
    if message.is_empty() {
        args.push("--no-edit");
        run(dir, &args).await?;
        return Ok(());
    }
    args.extend(["--cleanup=strip", "-F", "-"]);
    with_input(dir, &args, message).await
}

async fn has_head(dir: &Path) -> bool {
    run(dir, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .await
        .is_ok()
}

/// Paths come from clients, so they must stay inside the folder.
fn check_paths(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        bail!("no files given");
    }
    for path in paths {
        let inside = !path.is_empty()
            && Path::new(path)
                .components()
                .all(|part| matches!(part, Component::Normal(_)));
        if !inside {
            bail!("{path} isn't a path inside the folder");
        }
    }
    Ok(())
}

/// A patch for one hunk of one file, which `git apply` takes on stdin.
pub fn patch_for(path: &str, hunk: &Hunk) -> String {
    let old = quote(&format!("a/{path}"));
    let new = quote(&format!("b/{path}"));
    let mut patch = format!(
        "diff --git {old} {new}\n--- {old}\n+++ {new}\n@@ -{},{} +{},{} @@\n",
        hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
    );
    for line in &hunk.lines {
        patch.push_str(line);
        patch.push('\n');
    }
    patch
}

/// Git's C-style quoting, which `git apply` reads back, so any path survives the trip.
fn quote(path: &str) -> String {
    let mut quoted = String::from("\"");
    for c in path.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

async fn with_input(dir: &Path, args: &[&str], input: &str) -> Result<()> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        // Nothing may wait for a person at a terminal that isn't there.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to run git. Is it installed?")?;
    let mut stdin = child.stdin.take().context("git took no input")?;
    stdin.write_all(input.as_bytes()).await?;
    drop(stdin);
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::diff::read;
    use protocol::{DiffScope, FileDiff};

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.name", "t"]);
        git(repo, &["config", "user.email", "t@t"]);
        let lines: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        std::fs::write(repo.join("a.txt"), lines).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", "init"]);
        dir
    }

    async fn files(dir: &Path) -> Vec<FileDiff> {
        read(dir, &DiffScope::Uncommitted).await.unwrap().files
    }

    fn hunks(file: &FileDiff) -> Vec<Hunk> {
        match &file.body {
            FileBody::Text { hunks } => hunks.clone(),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn hunks_stage_unstage_and_discard_one_at_a_time() {
        let dir = repo();
        let repo = dir.path();
        let text = std::fs::read_to_string(repo.join("a.txt")).unwrap();
        let edited = text
            .replace("line 2\n", "line two\n")
            .replace("line 19\n", "line nineteen\n");
        std::fs::write(repo.join("a.txt"), edited).unwrap();

        let rest = hunks(&files(repo).await[0]);
        assert_eq!(rest.len(), 2);
        apply_hunk(repo, "a.txt", &rest[0], HunkAction::Stage)
            .await
            .unwrap();
        let now = files(repo).await;
        assert_eq!(now.len(), 2);
        assert!(now[0].staged);
        assert_eq!(hunks(&now[0]), [rest[0].clone()]);

        // The old hunk is gone from what isn't staged, so trying again is refused.
        let error = apply_hunk(repo, "a.txt", &rest[0], HunkAction::Stage)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("changed since"), "{error}");

        apply_hunk(repo, "a.txt", &hunks(&now[0])[0], HunkAction::Unstage)
            .await
            .unwrap();
        assert!(files(repo).await.iter().all(|f| !f.staged));

        let rest = hunks(&files(repo).await[0]);
        apply_hunk(repo, "a.txt", &rest[1], HunkAction::Discard)
            .await
            .unwrap();
        let text = std::fs::read_to_string(repo.join("a.txt")).unwrap();
        assert!(text.contains("line two\n") && text.contains("line 19\n"));
    }

    #[tokio::test]
    async fn files_stage_discard_and_commit() {
        let dir = repo();
        let repo = dir.path();
        std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
        std::fs::write(repo.join("new file.txt"), "new\n").unwrap();
        std::fs::write(repo.join("junk.txt"), "junk\n").unwrap();

        stage_files(repo, &["a.txt".into(), "new file.txt".into()])
            .await
            .unwrap();
        discard_files(repo, &["junk.txt".into()]).await.unwrap();
        assert!(!repo.join("junk.txt").exists());
        let staged: Vec<_> = files(repo)
            .await
            .into_iter()
            .map(|f| (f.path, f.staged))
            .collect();
        assert_eq!(
            staged,
            [("a.txt".into(), true), ("new file.txt".into(), true)]
        );

        commit(repo, "  Change things\n\nWith a body  ", false)
            .await
            .unwrap();
        let changes = read(repo, &DiffScope::Uncommitted).await.unwrap();
        assert!(changes.files.is_empty());
        assert_eq!(
            changes.head_message.as_deref(),
            Some("Change things\n\nWith a body")
        );

        commit(repo, "Better words", true).await.unwrap();
        let changes = read(repo, &DiffScope::Uncommitted).await.unwrap();
        assert_eq!(changes.head_message.as_deref(), Some("Better words"));
        assert!(commit(repo, " ", false).await.is_err());
    }

    #[tokio::test]
    async fn paths_must_stay_inside_the_folder() {
        let dir = repo();
        for path in ["../x", "/etc/passwd", "", "a/../../b"] {
            assert!(
                discard_files(dir.path(), &[path.into()]).await.is_err(),
                "{path} was allowed"
            );
        }
    }

    #[test]
    fn a_patch_quotes_its_paths() {
        let hunk = Hunk {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            context: String::new(),
            lines: vec!["-a".into(), "+b".into()],
        };
        assert_eq!(
            patch_for("x \"y\".txt", &hunk),
            "diff --git \"a/x \\\"y\\\".txt\" \"b/x \\\"y\\\".txt\"\n--- \"a/x \\\"y\\\".txt\"\n+++ \"b/x \\\"y\\\".txt\"\n@@ -1,1 +1,1 @@\n-a\n+b\n"
        );
    }
}
