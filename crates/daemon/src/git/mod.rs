//! Plain `git` calls for worktrees and diffs. Whether the daemon should use twig instead
//! is still an open question, and keeping this small leaves that open.

pub mod actions;
pub mod diff;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tokio::process::Command;

async fn git(dir: &Path, args: &[&std::ffi::OsStr]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .context("failed to run git. Is it installed?")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Returns the top level of the repository containing `path`.
pub async fn repo_root(path: &Path) -> Result<PathBuf> {
    let root = git(path, &["rev-parse".as_ref(), "--show-toplevel".as_ref()]).await?;
    Ok(PathBuf::from(root))
}

/// Makes `path`, which mustn't exist yet, a new repository. Topics branch from `HEAD`, so
/// it gets an empty first commit, by whoever the machine's git config says. Anything that
/// fails takes the folder away again, so trying again starts clean.
pub async fn create_repo(path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path.parent().context("a new folder needs a parent")?).await?;
    tokio::fs::create_dir(path).await?;
    let made = async {
        git(path, &["init".as_ref(), "-q".as_ref()]).await?;
        git(
            path,
            &[
                "commit".as_ref(),
                "-q".as_ref(),
                "--allow-empty".as_ref(),
                "-m".as_ref(),
                "Initial commit".as_ref(),
            ],
        )
        .await
        .context("couldn't make the first commit")
    }
    .await;
    if made.is_err() {
        let _ = tokio::fs::remove_dir_all(path).await;
    }
    made.map(drop)
}

/// How a clone logs in to GitHub over HTTPS.
pub enum CloneLogin {
    /// Whatever the machine's git config says.
    GitConfig,
    /// `gh`, like `gh repo clone` does, since `gh auth login` doesn't always set git up.
    Gh,
}

/// Clones `url` into `path`, which mustn't exist yet, telling `progress` what git is doing
/// and how far along it is. Git can't ask for a password, because nobody would see it ask, so
/// a missing login fails with git's message. Anything that fails takes the folder away again.
pub async fn clone(
    url: &str,
    path: &Path,
    login: CloneLogin,
    mut progress: impl FnMut(&str, u8),
) -> Result<()> {
    use tokio::io::AsyncReadExt;

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut command = Command::new("git");
    if matches!(login, CloneLogin::Gh) {
        // The empty value drops the helpers configured before it, as `gh` itself does.
        command.args([
            "-c",
            "credential.https://github.com.helper=",
            "-c",
            "credential.https://github.com.helper=!gh auth git-credential",
        ]);
    }
    let mut child = command
        .args(["clone", "--progress", "--"])
        .arg(url)
        .arg(path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to run git. Is it installed?")?;

    let mut stderr = child.stderr.take().context("git's output went missing")?;
    let mut said = Vec::new();
    let mut pending = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = stderr.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        pending.extend_from_slice(&buffer[..read]);
        // Git redraws its progress with carriage returns, and ends what it says with newlines.
        while let Some(end) = pending.iter().position(|&b| b == b'\r' || b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]);
            match clone_progress(&line) {
                Some((phase, percent)) => progress(phase, percent),
                None if !line.trim().is_empty() => said.push(line.trim().to_owned()),
                None => {}
            }
        }
    }
    said.extend(
        Some(String::from_utf8_lossy(&pending).trim().to_owned()).filter(|l| !l.is_empty()),
    );
    let status = child.wait().await?;
    if status.success() {
        return Ok(());
    }
    if tokio::fs::symlink_metadata(path).await.is_ok() {
        let _ = tokio::fs::remove_dir_all(path).await;
    }
    let reason = said
        .iter()
        .filter(|line| !line.starts_with("Cloning into"))
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    if reason.is_empty() {
        bail!("git clone failed with {status}");
    }
    bail!("{reason}");
}

/// Reads one of git's progress lines, like `Receiving objects:  42% (21/50)`, as what it's
/// doing and how far along it is.
fn clone_progress(line: &str) -> Option<(&str, u8)> {
    let line = line.trim().trim_start_matches("remote:").trim_start();
    let (phase, rest) = line.split_once(':')?;
    let percent = rest.trim_start().split_once('%')?.0;
    let percent: u8 = percent.trim().parse().ok()?;
    Some((phase.trim(), percent.min(100)))
}

/// The branch checked out in `repo`, or `HEAD` when it's detached.
pub async fn current_branch(repo: &Path) -> Result<String> {
    let branch = git(repo, &["branch".as_ref(), "--show-current".as_ref()]).await?;
    Ok(if branch.is_empty() {
        "HEAD".to_owned()
    } else {
        branch
    })
}

/// Creates a new branch from `start`, a commit or `HEAD`, checked out in a new worktree at
/// `path`.
pub async fn add_worktree(repo: &Path, path: &Path, branch: &str, start: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    git(
        repo,
        &[
            "worktree".as_ref(),
            "add".as_ref(),
            "-b".as_ref(),
            branch.as_ref(),
            path.as_os_str(),
            start.as_ref(),
        ],
    )
    .await
    .context("git worktree add failed")?;
    Ok(())
}

/// Checks out `branch` in a new worktree at `path`. Git takes a local branch, or else makes
/// one from the only remote that has it. With neither, it starts the branch from `HEAD`.
pub async fn restore_worktree(repo: &Path, path: &Path, branch: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let existing = git(
        repo,
        &[
            "worktree".as_ref(),
            "add".as_ref(),
            path.as_os_str(),
            branch.as_ref(),
        ],
    )
    .await;
    if existing.is_ok() {
        return Ok(());
    }
    add_worktree(repo, path, branch, "HEAD").await
}

/// The commit checked out in `dir`.
pub async fn head_commit(dir: &Path) -> Result<String> {
    git(dir, &["rev-parse".as_ref(), "HEAD".as_ref()]).await
}

/// Removes a worktree whatever it holds, then its branch whatever is on it. Callers ask first
/// with [`uncommitted`] and [`unpushed`]. A folder that's already gone is only forgotten.
pub async fn discard_worktree(repo: &Path, path: &Path, branch: &str) -> Result<()> {
    let removed = if path.exists() {
        git(
            repo,
            &[
                "worktree".as_ref(),
                "remove".as_ref(),
                "--force".as_ref(),
                "--force".as_ref(),
                path.as_os_str(),
            ],
        )
        .await
        .context("git worktree remove failed")
    } else {
        git(repo, &["worktree".as_ref(), "prune".as_ref()])
            .await
            .context("git worktree prune failed")
    };
    let deleted = git(
        repo,
        &[
            "branch".as_ref(),
            "-D".as_ref(),
            "--".as_ref(),
            branch.as_ref(),
        ],
    )
    .await;
    removed?;
    // A branch that was already deleted is what was wanted.
    if let Err(error) = deleted
        && branch_exists(repo, branch).await
    {
        return Err(error.context("git branch -D failed"));
    }
    Ok(())
}

pub(crate) async fn branch_exists(repo: &Path, branch: &str) -> bool {
    let reference = format!("refs/heads/{branch}");
    git(
        repo,
        &[
            "show-ref".as_ref(),
            "--verify".as_ref(),
            "--quiet".as_ref(),
            reference.as_ref(),
        ],
    )
    .await
    .is_ok()
}

/// Changed and untracked files in a worktree, leaving out ignored ones. A folder that's gone
/// has none.
pub async fn uncommitted(dir: &Path) -> Result<u32> {
    if !dir.exists() {
        return Ok(0);
    }
    let status = git(
        dir,
        &[
            "status".as_ref(),
            "--porcelain".as_ref(),
            "--untracked-files=all".as_ref(),
        ],
    )
    .await?;
    Ok(status.lines().filter(|line| !line.is_empty()).count() as u32)
}

/// Commits that deleting `branch` would lose, because no other branch or remote has them. A
/// branch that doesn't exist has none.
pub async fn unpushed(repo: &Path, branch: &str) -> Result<u32> {
    if !branch_exists(repo, branch).await {
        return Ok(0);
    }
    let reference = format!("refs/heads/{branch}");
    // Git wants this without `refs/heads/`, because it applies to `--branches`.
    let exclude = format!("--exclude={branch}");
    let count = git(
        repo,
        &[
            "rev-list".as_ref(),
            "--count".as_ref(),
            reference.as_ref(),
            "--not".as_ref(),
            exclude.as_ref(),
            "--branches".as_ref(),
            "--remotes".as_ref(),
        ],
    )
    .await?;
    count
        .parse()
        .with_context(|| format!("git rev-list printed {count}"))
}

/// Turns a topic name into something safe for a branch and directory name.
pub fn slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(40);
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "topic".to_owned()
    } else {
        slug.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_safe_for_branches_and_paths() {
        assert_eq!(slug("Fix the login page!"), "fix-the-login-page");
        assert_eq!(slug("--force ../../etc"), "force-etc");
        assert_eq!(slug("日本語"), "topic");
        assert_eq!(slug(""), "topic");
        assert!(slug(&"a b ".repeat(50)).len() <= 40);
        assert!(!slug(&"a b ".repeat(50)).ends_with('-'));
    }

    #[tokio::test]
    async fn worktree_is_created_on_a_new_branch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        ] {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        }

        let worktree = dir.path().join("nested/worktree");
        add_worktree(&repo, &worktree, "tectr/new", "HEAD")
            .await
            .unwrap();

        let branch = git(&worktree, &["branch".as_ref(), "--show-current".as_ref()])
            .await
            .unwrap();
        assert_eq!(branch, "tectr/new");
    }

    #[test]
    fn clone_progress_reads_what_git_is_doing() {
        assert_eq!(
            clone_progress("Receiving objects:  42% (21/50), 1.2 MiB | 3 MiB/s"),
            Some(("Receiving objects", 42))
        );
        assert_eq!(
            clone_progress("remote: Counting objects: 100% (7/7), done."),
            Some(("Counting objects", 100))
        );
        assert_eq!(clone_progress("Cloning into '/tmp/x'..."), None);
        assert_eq!(clone_progress("fatal: repository 'x' does not exist"), None);
        assert_eq!(
            clone_progress("remote: Enumerating objects: 7, done."),
            None
        );
    }

    #[tokio::test]
    async fn clone_copies_a_repo_and_cleans_up_after_failing() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        create_repo(&source).await.unwrap();

        let copy = dir.path().join("nested/copy");
        clone(
            &source.display().to_string(),
            &copy,
            CloneLogin::GitConfig,
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(
            head_commit(&copy).await.unwrap(),
            head_commit(&source).await.unwrap()
        );

        let failed = dir.path().join("failed");
        let error = clone(
            &dir.path().join("missing").display().to_string(),
            &failed,
            CloneLogin::GitConfig,
            |_, _| {},
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("does not exist"), "{error:#}");
        assert!(!failed.exists());
    }

    #[tokio::test]
    async fn create_repo_refuses_a_folder_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        assert!(create_repo(dir.path()).await.is_err());
        assert!(dir.path().exists());
    }

    #[tokio::test]
    async fn repo_root_rejects_a_directory_outside_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(repo_root(dir.path()).await.is_err());
    }
}
