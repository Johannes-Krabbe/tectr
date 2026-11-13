//! What changed in a folder, for diff tabs. Git's own diff is parsed rather than computed
//! here, so renames, the user's diff algorithm and its idea of what's ignored all carry over.

use std::path::Path;

use anyhow::{Context, Result, bail};
use protocol::{DiffScope, FileBody, FileDiff, FileStatus, Hunk};
use tokio::process::Command;

/// A file with more lines than this is sent without them.
const MAX_FILE_LINES: usize = 5_000;
/// An untracked file bigger than this isn't read.
const MAX_FILE_BYTES: u64 = 1 << 20;
/// Once a diff has this many lines, the files after are sent without theirs, which keeps one
/// answer well inside a websocket message.
const MAX_TOTAL_LINES: usize = 50_000;
/// Git calls a file binary when a NUL shows up this early.
const BINARY_PROBE: usize = 8_000;

/// What a folder's diff shows for one scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes {
    pub base: Option<String>,
    /// The last commit's message, which amending starts from. Only the uncommitted scope reads
    /// it, because only it can commit.
    pub head_message: Option<String>,
    pub files: Vec<FileDiff>,
}

/// Reads what changed in `dir`. Fails on a scope this daemon doesn't know, and on the branch
/// scope when there's nothing to compare with.
pub async fn read(dir: &Path, scope: &DiffScope) -> Result<Changes> {
    let (base, mut files) = match scope {
        DiffScope::Uncommitted => {
            let staged = parse(&run(dir, &diff_args(&["--cached"])).await?, true);
            let mut rest = parse(&run(dir, &diff_args(&[])).await?, false);
            rest.extend(untracked(dir).await?);
            rest.sort_by(|a, b| a.path.cmp(&b.path));
            (None, staged.into_iter().chain(rest).collect::<Vec<_>>())
        }
        DiffScope::Branch => {
            let Some(base) = default_base(dir).await else {
                bail!("there's no main or master branch to compare with");
            };
            let fork = run(dir, &["merge-base", "HEAD", &base])
                .await
                .with_context(|| format!("this branch has nothing in common with {base}"))?;
            let fork = fork.trim();
            let mut files = parse(&run(dir, &diff_args(&[fork])).await?, false);
            files.extend(untracked(dir).await?);
            files.sort_by(|a, b| a.path.cmp(&b.path));
            (Some(base), files)
        }
        DiffScope::Unknown => bail!("this daemon doesn't know that kind of diff"),
    };
    limit_total(&mut files);
    // A repository with no commits yet has nothing to amend.
    let head_message = match scope {
        DiffScope::Uncommitted => run(dir, &["log", "-1", "--format=%B"])
            .await
            .ok()
            .map(|message| message.trim_end().to_owned()),
        _ => None,
    };
    Ok(Changes {
        base,
        head_message,
        files,
    })
}

/// The branch a topic's work is compared with: the remote's default branch, as a local branch
/// when there is one, or else `main` or `master`.
pub async fn default_base(dir: &Path) -> Option<String> {
    if let Ok(remote) = run(
        dir,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .await
    {
        let remote = remote.trim();
        if let Some(name) = remote.strip_prefix("origin/")
            && super::branch_exists(dir, name).await
        {
            return Some(name.to_owned());
        }
        if !remote.is_empty() {
            return Some(remote.to_owned());
        }
    }
    for name in ["main", "master"] {
        if super::branch_exists(dir, name).await {
            return Some(name.to_owned());
        }
    }
    None
}

/// Settings people have that would change what `diff` prints are overridden, so the output
/// always parses the same way.
pub(super) fn diff_args<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "diff",
        "--no-color",
        "--no-ext-diff",
        // What a textconv filter prints can't be staged back, and staging comes next.
        "--no-textconv",
        "--find-renames",
        "--no-relative",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--submodule=short",
    ];
    args.extend_from_slice(extra);
    args
}

/// Runs git and keeps its output exactly, unlike `super::git`, which trims it.
pub(super) async fn run(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotePath=false"])
        .args(args)
        // Reading must never write the index: that would be a change to the folder, which
        // would read it again.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to run git. Is it installed?")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Untracked files as if they were added, which is what staging them would do.
async fn untracked(dir: &Path) -> Result<Vec<FileDiff>> {
    let listed = run(dir, &["ls-files", "--others", "--exclude-standard", "-z"]).await?;
    let mut files = vec![];
    // A folder listed with a slash is a repository of its own, which git won't look into.
    for path in listed
        .split('\0')
        .filter(|p| !p.is_empty() && !p.ends_with('/'))
    {
        files.push(untracked_file(dir, path).await);
    }
    Ok(files)
}

async fn untracked_file(dir: &Path, path: &str) -> FileDiff {
    let file = |added, body| FileDiff {
        path: path.to_owned(),
        old_path: None,
        status: FileStatus::Untracked,
        staged: false,
        added,
        removed: 0,
        body,
    };
    let full = dir.join(path);
    let Ok(meta) = tokio::fs::symlink_metadata(&full).await else {
        // Gone since git listed it, which the next read will show.
        return file(0, FileBody::Text { hunks: vec![] });
    };
    // Git keeps a link as the path it points to.
    let bytes = if meta.is_symlink() {
        match tokio::fs::read_link(&full).await {
            Ok(target) => target.to_string_lossy().into_owned().into_bytes(),
            Err(_) => vec![],
        }
    } else if meta.len() > MAX_FILE_BYTES {
        return file(0, FileBody::TooLarge);
    } else {
        tokio::fs::read(&full).await.unwrap_or_default()
    };
    if bytes[..bytes.len().min(BINARY_PROBE)].contains(&0) {
        return file(0, FileBody::Binary);
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text.lines().map(|line| format!("+{line}")).collect();
    let count = lines.len() as u32;
    if lines.len() > MAX_FILE_LINES {
        return file(count, FileBody::TooLarge);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        lines.push("\\ No newline at end of file".to_owned());
    }
    let hunks = if count == 0 {
        vec![]
    } else {
        vec![Hunk {
            old_start: 0,
            old_lines: 0,
            new_start: 1,
            new_lines: count,
            context: String::new(),
            lines,
        }]
    };
    file(count, FileBody::Text { hunks })
}

/// Files past the budget keep their counts and lose their lines.
fn limit_total(files: &mut [FileDiff]) {
    let mut total = 0;
    for file in files {
        if let FileBody::Text { hunks } = &file.body {
            let lines: usize = hunks.iter().map(|h| h.lines.len()).sum();
            total += lines;
            if total > MAX_TOTAL_LINES {
                file.body = FileBody::TooLarge;
            }
        }
    }
}

/// Reads the output of `git diff`. Every file's lines go to whoever reads them, so this never
/// fails: a line it doesn't understand is left out.
pub fn parse(patch: &str, staged: bool) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = vec![];
    // Lines a hunk still has to come, on each side. A line that looks like a header is content
    // while either is above zero.
    let mut old_left = 0u32;
    let mut new_left = 0u32;
    for line in patch.lines() {
        if old_left > 0 || new_left > 0 {
            let Some(file) = files.last_mut() else {
                break;
            };
            let FileBody::Text { hunks } = &mut file.body else {
                break;
            };
            let Some(hunk) = hunks.last_mut() else {
                break;
            };
            match line.as_bytes().first() {
                Some(b'+') => {
                    new_left = new_left.saturating_sub(1);
                    file.added += 1;
                }
                Some(b'-') => {
                    old_left = old_left.saturating_sub(1);
                    file.removed += 1;
                }
                Some(b'\\') => {}
                // A blank line is a context line whose leading space was lost along the way.
                _ => {
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                }
            }
            hunk.lines.push(if line.is_empty() {
                " ".to_owned()
            } else {
                line.to_owned()
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let (old, new) = header_paths(rest);
            files.push(FileDiff {
                path: new.clone(),
                old_path: (old != new).then_some(old),
                status: FileStatus::Modified,
                staged,
                added: 0,
                removed: 0,
                body: FileBody::Text { hunks: vec![] },
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if line.starts_with('\\') {
            // Says the last line of the hunk before ends its file without a newline.
            if let FileBody::Text { hunks } = &mut file.body
                && let Some(hunk) = hunks.last_mut()
            {
                hunk.lines.push(line.to_owned());
            }
        } else if let Some(header) = line.strip_prefix("@@ ") {
            if let Some(hunk) = hunk_header(header) {
                old_left = hunk.old_lines;
                new_left = hunk.new_lines;
                if let FileBody::Text { hunks } = &mut file.body {
                    hunks.push(hunk);
                }
            }
        } else if line.starts_with("new file mode") {
            file.status = FileStatus::Added;
        } else if line.starts_with("deleted file mode") {
            file.status = FileStatus::Deleted;
        } else if let Some(from) = line.strip_prefix("rename from ") {
            file.status = FileStatus::Renamed;
            file.old_path = Some(unquote(from));
        } else if let Some(to) = line.strip_prefix("rename to ") {
            file.path = unquote(to);
        } else if let Some(from) = line.strip_prefix("copy from ") {
            file.status = FileStatus::Copied;
            file.old_path = Some(unquote(from));
        } else if let Some(to) = line.strip_prefix("copy to ") {
            file.path = unquote(to);
        } else if let Some(old) = line.strip_prefix("--- ") {
            if let Some(old) = side_path(old, "a/") {
                if file.status != FileStatus::Renamed && file.status != FileStatus::Copied {
                    file.old_path = None;
                }
                if file.status == FileStatus::Deleted {
                    file.path = old;
                }
            }
        } else if let Some(new) = line.strip_prefix("+++ ") {
            if let Some(new) = side_path(new, "b/") {
                file.path = new;
            }
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            file.body = FileBody::Binary;
        }
    }
    for file in &mut files {
        if file.status != FileStatus::Renamed && file.status != FileStatus::Copied {
            file.old_path = None;
        }
        if let FileBody::Text { hunks } = &file.body
            && hunks.iter().map(|h| h.lines.len()).sum::<usize>() > MAX_FILE_LINES
        {
            file.body = FileBody::TooLarge;
        }
    }
    files
}

/// A path after `--- ` or `+++ `, without its prefix. `/dev/null` has none.
fn side_path(text: &str, prefix: &str) -> Option<String> {
    // Git ends a path with a tab when it has spaces at the end, for other tools.
    let text = text.strip_suffix('\t').unwrap_or(text);
    if text == "/dev/null" {
        return None;
    }
    let path = unquote(text);
    Some(path.strip_prefix(prefix).map(str::to_owned).unwrap_or(path))
}

/// The two paths in `diff --git a/x b/y`. Only binary files and mode changes need these,
/// because everything else has `---` and `+++` lines or renames, which are unambiguous.
fn header_paths(rest: &str) -> (String, String) {
    if rest.starts_with('"')
        && let Some((old, new)) = split_quoted(rest)
    {
        return (strip(&old, "a/"), strip(&new, "b/"));
    }
    // Unrenamed, both halves are the same path, which may itself hold " b/".
    let len = rest.len();
    if len >= 5 && (len - 5).is_multiple_of(2) {
        let n = (len - 5) / 2;
        if let (Some(old), Some(new)) = (rest.get(2..2 + n), rest.get(len - n..))
            && old == new
            && rest.get(2 + n..len - n) == Some(" b/")
        {
            return (old.to_owned(), new.to_owned());
        }
    }
    match rest.split_once(" b/") {
        Some((old, new)) => (strip(old, "a/"), new.to_owned()),
        None => (rest.to_owned(), rest.to_owned()),
    }
}

fn strip(path: &str, prefix: &str) -> String {
    path.strip_prefix(prefix).unwrap_or(path).to_owned()
}

/// A quoted path followed by another path, quoted or not.
fn split_quoted(text: &str) -> Option<(String, String)> {
    let end = closing_quote(text)?;
    let (first, rest) = text.split_at(end + 1);
    let second = rest.strip_prefix(' ')?;
    Some((unquote(first), unquote(second)))
}

fn closing_quote(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Undoes git's C-style quoting of paths with unusual characters.
fn unquote(text: &str) -> String {
    let Some(inner) = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return text.to_owned();
    };
    let mut bytes = Vec::with_capacity(inner.len());
    let mut chars = inner.bytes().peekable();
    while let Some(byte) = chars.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        match chars.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'a') => bytes.push(7),
            Some(b'b') => bytes.push(8),
            Some(b'f') => bytes.push(12),
            Some(b'v') => bytes.push(11),
            Some(digit @ b'0'..=b'7') => {
                let mut value = u32::from(digit - b'0');
                for _ in 0..2 {
                    match chars.peek() {
                        Some(next @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(next - b'0');
                            chars.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(value as u8);
            }
            Some(other) => bytes.push(other),
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `-1,3 +1,4 @@ fn main()`, after the leading `@@ `.
fn hunk_header(text: &str) -> Option<Hunk> {
    let (ranges, context) = text.split_once("@@")?;
    let mut parts = ranges.split_whitespace();
    let (old_start, old_lines) = range(parts.next()?.strip_prefix('-')?)?;
    let (new_start, new_lines) = range(parts.next()?.strip_prefix('+')?)?;
    Some(Hunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        context: context.trim().to_owned(),
        lines: vec![],
    })
}

/// `12,3` or `12`, which means one line.
fn range(text: &str) -> Option<(u32, u32)> {
    match text.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((text.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunks(file: &FileDiff) -> &[Hunk] {
        match &file.body {
            FileBody::Text { hunks } => hunks,
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn reads_a_modified_file_with_two_hunks_and_counts_its_lines() {
        let patch = "\
diff --git a/src/main.rs b/src/main.rs
index 1111111..2222222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@ fn main() {
 one
-two
+deux
+trois
 three
@@ -10 +11 @@
--- a removed line that looks like a header
+++ an added one too
";
        let files = parse(patch, true);
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.path, "src/main.rs");
        assert_eq!(file.old_path, None);
        assert_eq!(file.status, FileStatus::Modified);
        assert!(file.staged);
        assert_eq!((file.added, file.removed), (3, 2));
        let hunks = hunks(file);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].context, "fn main() {");
        assert_eq!(
            hunks[0].lines,
            [" one", "-two", "+deux", "+trois", " three"]
        );
        assert_eq!(
            (
                hunks[1].old_start,
                hunks[1].old_lines,
                hunks[1].new_start,
                hunks[1].new_lines
            ),
            (10, 1, 11, 1)
        );
        assert_eq!(
            hunks[1].lines,
            [
                "--- a removed line that looks like a header",
                "+++ an added one too"
            ]
        );
    }

    #[test]
    fn reads_added_deleted_renamed_binary_and_mode_only_files() {
        let patch = "\
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
\\ No newline at end of file
diff --git a/old.txt b/old.txt
deleted file mode 100644
index 1111111..0000000
--- a/old.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/a b/c d.txt b/a b/e f.txt
similarity index 90%
rename from a b/c d.txt
rename to a b/e f.txt
index 1111111..2222222 100644
--- a/a b/c d.txt
+++ b/a b/e f.txt
@@ -1 +1 @@
-x
+y
diff --git a/logo.png b/logo.png
index 1111111..2222222 100644
Binary files a/logo.png and b/logo.png differ
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
";
        let files = parse(patch, false);
        let summary: Vec<_> = files
            .iter()
            .map(|f| (f.path.as_str(), f.old_path.as_deref(), f.status))
            .collect();
        assert_eq!(
            summary,
            [
                ("new.txt", None, FileStatus::Added),
                ("old.txt", None, FileStatus::Deleted),
                ("a b/e f.txt", Some("a b/c d.txt"), FileStatus::Renamed),
                ("logo.png", None, FileStatus::Modified),
                ("run.sh", None, FileStatus::Modified),
            ]
        );
        assert_eq!(
            hunks(&files[0])[0].lines,
            ["+hello", "\\ No newline at end of file"]
        );
        assert_eq!(files[3].body, FileBody::Binary);
        assert!(hunks(&files[4]).is_empty());
    }

    #[test]
    fn unquotes_paths_git_quoted() {
        assert_eq!(unquote("\"tab\\there\""), "tab\there");
        assert_eq!(unquote("\"caf\\303\\251\""), "café");
        assert_eq!(unquote("plain"), "plain");
        let (old, new) = header_paths("\"a/x\\\"y\" \"b/x\\\"y\"");
        assert_eq!((old.as_str(), new.as_str()), ("x\"y", "x\"y"));
        let (old, new) = header_paths("a/has b/inside b/has b/inside");
        assert_eq!(
            (old.as_str(), new.as_str()),
            ("has b/inside", "has b/inside")
        );
    }

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

    #[tokio::test]
    async fn reads_both_scopes_of_a_real_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("kept.txt"), "one\ntwo\n").unwrap();
        std::fs::write(repo.join("staged.txt"), "a\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", "init"]);
        git(repo, &["checkout", "-q", "-b", "topic"]);
        std::fs::write(repo.join("committed.txt"), "on the branch\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-q", "-m", "work"]);
        std::fs::write(repo.join("staged.txt"), "b\n").unwrap();
        git(repo, &["add", "staged.txt"]);
        std::fs::write(repo.join("staged.txt"), "c\n").unwrap();
        std::fs::write(repo.join("kept.txt"), "one\n2\n").unwrap();
        std::fs::write(repo.join("new.txt"), "fresh").unwrap();
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(repo.join("ignored.txt"), "no").unwrap();

        let uncommitted = read(repo, &DiffScope::Uncommitted).await.unwrap();
        let listed: Vec<_> = uncommitted
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.staged, f.status))
            .collect();
        assert_eq!(
            listed,
            [
                ("staged.txt", true, FileStatus::Modified),
                (".gitignore", false, FileStatus::Untracked),
                ("kept.txt", false, FileStatus::Modified),
                ("new.txt", false, FileStatus::Untracked),
                ("staged.txt", false, FileStatus::Modified),
            ]
        );
        assert_eq!(uncommitted.base, None);
        let new = &uncommitted.files[3];
        assert_eq!(
            hunks(new)[0].lines,
            ["+fresh", "\\ No newline at end of file"]
        );
        assert_eq!(new.added, 1);

        let branch = read(repo, &DiffScope::Branch).await.unwrap();
        assert_eq!(branch.base.as_deref(), Some("main"));
        let listed: Vec<_> = branch.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            listed,
            [
                ".gitignore",
                "committed.txt",
                "kept.txt",
                "new.txt",
                "staged.txt"
            ]
        );
        let staged = branch
            .files
            .iter()
            .find(|f| f.path == "staged.txt")
            .unwrap();
        assert_eq!(hunks(staged)[0].lines, ["-a", "+c"]);
    }

    #[tokio::test]
    async fn the_branch_scope_needs_something_to_compare_with() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "trunk"]);
        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
        let error = read(dir.path(), &DiffScope::Branch).await.unwrap_err();
        assert!(error.to_string().contains("no main or master"), "{error}");
    }
}
