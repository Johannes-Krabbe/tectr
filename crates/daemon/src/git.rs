//! Plain `git` calls for worktrees. Whether the daemon should use twig instead
//! is still an open question, and keeping this small leaves that open.

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

/// Creates a new branch from `HEAD`, checked out in a new worktree at `path`.
pub async fn add_worktree(repo: &Path, path: &Path, branch: &str) -> Result<()> {
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
            "HEAD".as_ref(),
        ],
    )
    .await
    .context("git worktree add failed")?;
    Ok(())
}

/// Removes a worktree, and its branch if everything on the branch is already merged. Git refuses
/// a worktree with uncommitted changes and a branch with commits nowhere else, and those refusals
/// are what keeps someone's work, so neither is ever forced.
pub async fn remove_worktree(repo: &Path, path: &Path, branch: &str) -> Result<()> {
    git(
        repo,
        &["worktree".as_ref(), "remove".as_ref(), path.as_os_str()],
    )
    .await
    .context("git worktree remove failed")?;
    let _ = git(repo, &["branch".as_ref(), "-d".as_ref(), branch.as_ref()]).await;
    Ok(())
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
        add_worktree(&repo, &worktree, "tectr/new").await.unwrap();

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
