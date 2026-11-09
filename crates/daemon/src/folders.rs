//! Paths as people type them, and the folders offered while they do. The home folder and
//! the disk belong to the daemon's machine, which is why this isn't in a client.

use std::path::{Path, PathBuf};
use std::{fs, io};

use protocol::Folder;

/// Enough for any folder someone keeps repositories in, and small enough to send on every
/// new folder typed. Folders past it, in name order, can still be typed by hand.
const MAX_FOLDERS: usize = 1_000;

// macOS asks the person before anything looks inside these, and listing the home folder
// shouldn't set off three prompts. Going into one of them is asking to look.
const ASKS_PERMISSION: &[&str] = if cfg!(target_os = "macos") {
    &["Desktop", "Documents", "Downloads"]
} else {
    &[]
};

/// Turns a typed path into an absolute one. `None` means it's neither absolute nor under
/// `~`. Other people's homes, like `~root`, aren't supported.
pub fn resolve(path: &str, home: Option<&Path>) -> Option<PathBuf> {
    let resolved = match (path.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_owned(),
        (Some(rest), Some(home)) if rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(path),
    };
    resolved.is_absolute().then_some(resolved)
}

/// The folders directly inside `dir`, sorted by name ignoring case. Symlinks to folders
/// count as folders.
pub fn list(dir: &Path, home: Option<&Path>) -> io::Result<Vec<Folder>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(vec![]);
        }
        Err(error) => return Err(error),
    };

    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| {
                kind.is_dir()
                    || (kind.is_symlink()
                        && fs::metadata(entry.path()).is_ok_and(|target| target.is_dir()))
            })
        })
        // A name that isn't valid UTF-8 couldn't be sent back as a path anyway.
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
    names.truncate(MAX_FOLDERS);

    let in_home = home.is_some_and(|home| home == dir);
    Ok(names
        .into_iter()
        .map(|name| {
            let path = dir.join(&name);
            let may_look = !(in_home && ASKS_PERMISSION.contains(&name.as_str()));
            Folder {
                is_repo: may_look && path.join(".git").symlink_metadata().is_ok(),
                path: path.display().to_string(),
                name,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(path: &str) -> Option<String> {
        resolve(path, Some(Path::new("/home/me"))).map(|path| path.display().to_string())
    }

    #[test]
    fn a_tilde_means_the_home_folder() {
        assert_eq!(resolved("~").as_deref(), Some("/home/me"));
        assert_eq!(resolved("~/").as_deref(), Some("/home/me/"));
        assert_eq!(
            resolved("~/Downloads").as_deref(),
            Some("/home/me/Downloads")
        );
        assert_eq!(resolved("~/a/b/").as_deref(), Some("/home/me/a/b/"));
    }

    #[test]
    fn absolute_paths_are_left_alone() {
        assert_eq!(resolved("/srv/repo").as_deref(), Some("/srv/repo"));
        assert_eq!(resolved("/srv/~/repo").as_deref(), Some("/srv/~/repo"));
    }

    #[test]
    fn anything_else_is_refused() {
        assert_eq!(resolved("repo"), None);
        assert_eq!(resolved("./repo"), None);
        assert_eq!(resolved(""), None);
        assert_eq!(resolved("~root"), None);
        assert_eq!(resolved("~root/repo"), None);
        assert_eq!(resolve("~/repo", None), None);
    }

    fn names(folders: &[Folder]) -> Vec<&str> {
        folders.iter().map(|folder| folder.name.as_str()).collect()
    }

    #[test]
    fn only_folders_are_listed_sorted_ignoring_case() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["beta", "Alpha", ".hidden"] {
            fs::create_dir(dir.path().join(name)).unwrap();
        }
        fs::write(dir.path().join("a-file"), "").unwrap();

        let folders = list(dir.path(), None).unwrap();

        assert_eq!(names(&folders), [".hidden", "Alpha", "beta"]);
        assert_eq!(
            folders[1].path,
            dir.path().join("Alpha").display().to_string()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_counts_when_it_points_at_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("real")).unwrap();
        fs::write(dir.path().join("a-file"), "").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("to-folder")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("a-file"), dir.path().join("to-file")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("gone"), dir.path().join("broken")).unwrap();

        let folders = list(dir.path(), None).unwrap();

        assert_eq!(names(&folders), ["real", "to-folder"]);
    }

    #[test]
    fn a_folder_or_file_with_a_git_entry_is_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("repo/.git")).unwrap();
        // A worktree's .git is a file.
        fs::create_dir(dir.path().join("worktree")).unwrap();
        fs::write(dir.path().join("worktree/.git"), "gitdir: elsewhere").unwrap();
        fs::create_dir(dir.path().join("plain")).unwrap();

        let folders = list(dir.path(), None).unwrap();

        let repos: Vec<_> = folders
            .iter()
            .map(|f| (f.name.as_str(), f.is_repo))
            .collect();
        assert_eq!(
            repos,
            [("plain", false), ("repo", true), ("worktree", true)]
        );
    }

    #[test]
    fn a_missing_folder_or_a_file_has_no_folders() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a-file"), "").unwrap();

        assert_eq!(list(&dir.path().join("gone"), None).unwrap(), []);
        assert_eq!(list(&dir.path().join("a-file"), None).unwrap(), []);
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cant_be_read_is_an_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let readable_anyway = fs::read_dir(&locked).is_ok(); // True when running as root.

        let result = list(&locked, None);

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(result.is_err(), !readable_anyway);
    }

    #[test]
    fn long_listings_stop_at_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..=MAX_FOLDERS {
            fs::create_dir(dir.path().join(format!("{index:05}"))).unwrap();
        }

        let folders = list(dir.path(), None).unwrap();

        assert_eq!(folders.len(), MAX_FOLDERS);
        assert_eq!(folders[0].name, "00000");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn listing_the_home_folder_never_looks_inside_the_protected_ones() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("Downloads/.git")).unwrap();
        fs::create_dir_all(home.path().join("Developer/.git")).unwrap();

        let in_home = list(home.path(), Some(home.path())).unwrap();
        let elsewhere = list(home.path(), None).unwrap();

        let repos = |folders: &[Folder]| -> Vec<bool> {
            folders.iter().map(|folder| folder.is_repo).collect()
        };
        // Sorted, that's Developer and then Downloads.
        assert_eq!(repos(&in_home), [true, false]);
        assert_eq!(repos(&elsewhere), [true, true]);
    }
}
