//! What a client keeps on disk to reach daemons on other machines: its own key and the
//! daemons it paired with. The CLI and the desktop app on one machine share it, so pairing
//! once lets both in.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use iroh::SecretKey;
use protocol::Channel;

pub struct ClientStore {
    dir: PathBuf,
}

impl ClientStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `<data dir>/<channel>/client`, next to the daemon's data, so dev builds never use
    /// the installed app's pairings.
    pub fn default_dir() -> Option<PathBuf> {
        Some(
            dirs::data_dir()?
                .join(Channel::current().dir_name())
                .join("client"),
        )
    }

    /// This client's key, made the first time it's needed.
    pub fn key(&self) -> io::Result<SecretKey> {
        let path = self.dir.join("key");
        match fs::read_to_string(&path) {
            Ok(text) => text.trim().parse().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is damaged", path.display()),
                )
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let key = SecretKey::generate();
                let text: String = key
                    .to_bytes()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                self.write_private(&path, &text)?;
                Ok(key)
            }
            Err(error) => Err(error),
        }
    }

    /// The URLs of the daemons this client paired with, in the order they were added.
    pub fn daemons(&self) -> Vec<String> {
        fs::read_to_string(self.daemons_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn add_daemon(&self, url: &str) -> io::Result<()> {
        let mut daemons = self.daemons();
        if !daemons.iter().any(|known| known == url) {
            daemons.push(url.to_owned());
        }
        self.save_daemons(&daemons)
    }

    pub fn remove_daemon(&self, url: &str) -> io::Result<()> {
        let mut daemons = self.daemons();
        daemons.retain(|known| known != url);
        self.save_daemons(&daemons)
    }

    fn daemons_path(&self) -> PathBuf {
        self.dir.join("daemons.json")
    }

    fn save_daemons(&self, daemons: &[String]) -> io::Result<()> {
        let text = serde_json::to_string_pretty(daemons).map_err(io::Error::other)?;
        self.write_private(&self.daemons_path(), &text)
    }

    /// Only this user can read either file: the key is what lets this client in.
    fn write_private(&self, path: &Path, text: &str) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let temp = path.with_extension("tmp");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        // Replacing in one step means a crash can't leave half a file.
        fs::rename(&temp, path)
    }
}

/// What a daemon lists this client as when it pairs.
pub fn this_machine_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_and_daemons_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = ClientStore::new(dir.path().join("client"));
        assert!(store.daemons().is_empty());
        let key = store.key().unwrap();
        assert_eq!(store.key().unwrap().public(), key.public());

        store.add_daemon("iroh://a").unwrap();
        store.add_daemon("iroh://b").unwrap();
        store.add_daemon("iroh://a").unwrap();
        assert_eq!(store.daemons(), ["iroh://a", "iroh://b"]);
        store.remove_daemon("iroh://a").unwrap();
        assert_eq!(
            ClientStore::new(dir.path().join("client")).daemons(),
            ["iroh://b"]
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join("client/key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
