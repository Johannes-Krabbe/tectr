//! Plain `git` calls for worktrees and diffs. Whether the daemon should use twig instead
//! is still an open question, and keeping this small leaves that open.

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

    #[tokio::test]
    async fn repo_root_rejects_a_directory_outside_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(repo_root(dir.path()).await.is_err());
    }
}
