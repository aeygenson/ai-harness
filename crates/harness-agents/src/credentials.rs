//! Login tokens, stored outside every project: `~/.harness/credentials/<agent>/`.
//!
//! A token is a password: it never goes into git, a handoff, a log or a debug print.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const TOKEN_FILE: &str = "oauth-token";

/// A secret string. `{:?}` prints `Secret(***)`, so it cannot leak into logs by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// The only way to read the value, so every use is easy to find.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// `~/.harness/credentials`.
pub fn default_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".harness/credentials"))
}

/// Saves the token of `agent` (for example `claude`), readable only by Lisa.
pub fn save_token(dir: &Path, agent: &str, token: &Secret) -> io::Result<PathBuf> {
    let agent_dir = dir.join(agent);
    fs::create_dir_all(&agent_dir)?;
    let path = agent_dir.join(TOKEN_FILE);
    write_private(&path, token.expose().trim())?;
    Ok(path)
}

pub fn load_token(dir: &Path, agent: &str) -> io::Result<Secret> {
    let text = fs::read_to_string(dir.join(agent).join(TOKEN_FILE))?;
    Ok(Secret::new(text.trim()))
}

/// Creates the file with permissions 600 (owner reads and writes, nobody else),
/// before any secret is written into it.
#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // `mode` only applies to a new file; an old one might be readable by others.
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> io::Result<()> {
    fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_the_secret() {
        let secret = Secret::new("sk-very-secret");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
    }

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = save_token(dir.path(), "claude", &Secret::new(" token-1\n")).unwrap();
        assert_eq!(
            load_token(dir.path(), "claude").unwrap().expose(),
            "token-1"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_missing_token_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_token(dir.path(), "claude").is_err());
    }
}
