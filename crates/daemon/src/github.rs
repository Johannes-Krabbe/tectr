//! The GitHub repositories the machine's `gh` can see, for cloning one as a workspace. tectr
//! has no GitHub login of its own: whoever `gh` is logged in as on this machine is who it
//! lists for, and a machine without `gh` just has none to offer.

use std::time::Duration;

use protocol::Repo;
use serde::Deserialize;
use tokio::process::Command;

/// Listing a hundred repositories takes a few seconds, and a stuck network shouldn't hold a
/// request open forever.
const LIST_TIMEOUT: Duration = Duration::from_secs(60);

/// Everything the person owns, collaborates on, or can see through an organization, most
/// recently pushed first. `Err` says why there's nothing, to be shown as is.
pub async fn list_repos() -> Result<Vec<Repo>, String> {
    let listed = gh(&[
        "api",
        "--paginate",
        "user/repos?per_page=100&sort=pushed&affiliation=owner,collaborator,organization_member",
        "--jq",
        ".[] | {full_name, name, clone_url, ssh_url}",
    ]);
    let protocol = gh(&["config", "get", "git_protocol", "--host", "github.com"]);
    let (listed, protocol) = tokio::join!(listed, protocol);
    let ssh = protocol.is_ok_and(|protocol| protocol.trim() == "ssh");
    Ok(parse_repos(&listed?, ssh))
}

async fn gh(args: &[&str]) -> Result<String, String> {
    let run = Command::new("gh")
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(LIST_TIMEOUT, run).await {
        Err(_) => return Err("gh took too long to answer".to_owned()),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err("gh isn't installed on this device".to_owned());
        }
        Ok(Err(error)) => return Err(format!("couldn't run gh: {error}")),
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return Err(if said.contains("gh auth login") {
            "gh isn't logged in on this device. Run gh auth login there".to_owned()
        } else {
            let said = said.trim();
            if said.is_empty() {
                format!("gh failed with {}", output.status)
            } else {
                format!("gh: {said}")
            }
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Deserialize)]
struct Listed {
    full_name: String,
    name: String,
    clone_url: String,
    ssh_url: String,
}

/// Reads one repository per line, as the `--jq` filter writes them. A line that isn't one is
/// left out rather than losing the rest.
fn parse_repos(output: &str, ssh: bool) -> Vec<Repo> {
    let mut repos: Vec<Repo> = Vec::new();
    for listed in output
        .lines()
        .filter_map(|line| serde_json::from_str::<Listed>(line).ok())
    {
        // Pages can shift while they're read, which would list one twice.
        if repos.iter().any(|repo| repo.full_name == listed.full_name) {
            continue;
        }
        repos.push(Repo {
            url: if ssh {
                listed.ssh_url
            } else {
                listed.clone_url
            },
            full_name: listed.full_name,
            name: listed.name,
        });
    }
    repos
}

/// Whether `url` is GitHub over HTTPS, the one kind of address `gh` can log git in to.
pub fn is_github_https(url: &str) -> bool {
    url.starts_with("https://github.com/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTED: &str = r#"{"clone_url":"https://github.com/neotaste/neotaste-monorepo.git","full_name":"neotaste/neotaste-monorepo","name":"neotaste-monorepo","ssh_url":"git@github.com:neotaste/neotaste-monorepo.git"}
{"clone_url":"https://github.com/me/tectr.git","full_name":"me/tectr","name":"tectr","ssh_url":"git@github.com:me/tectr.git"}
not json
{"clone_url":"https://github.com/me/tectr.git","full_name":"me/tectr","name":"tectr","ssh_url":"git@github.com:me/tectr.git"}
"#;

    #[test]
    fn repos_keep_their_order_once_each() {
        let repos = parse_repos(LISTED, false);
        let names: Vec<&str> = repos.iter().map(|r| r.full_name.as_str()).collect();
        assert_eq!(names, ["neotaste/neotaste-monorepo", "me/tectr"]);
        assert_eq!(repos[1].name, "tectr");
        assert_eq!(repos[1].url, "https://github.com/me/tectr.git");
    }

    #[test]
    fn repos_clone_over_the_protocol_gh_uses() {
        let repos = parse_repos(LISTED, true);
        assert_eq!(repos[1].url, "git@github.com:me/tectr.git");
    }

    #[test]
    fn only_github_over_https_logs_in_through_gh() {
        assert!(is_github_https("https://github.com/me/tectr.git"));
        assert!(!is_github_https("git@github.com:me/tectr.git"));
        assert!(!is_github_https("https://gitlab.com/me/tectr.git"));
    }
}
