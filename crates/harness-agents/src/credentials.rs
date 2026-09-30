//! Login tokens, stored outside every project: `~/.harness/credentials/<agent>/`.
//!
//! A token is a password: it never goes into git, a handoff, a log or a debug print.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const TOKEN_FILE: &str = "oauth-token";

/// Lives in `harness_core`, because MCP server settings carry secrets too.
pub use harness_core::secret::Secret;

/// `~/.harness/credentials`.
pub fn default_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".harness/credentials"))
}

/// Saves the token of `agent` (for example `claude`), readable only by Lisa.
pub fn save_token(dir: &Path, agent: &str, token: &Secret) -> io::Result<PathBuf> {
    let agent_dir = dir.join(agent);
    fs::create_dir_all(&agent_dir)?;
    let path = agent_dir.join(TOKEN_FILE);
    write_private(&path, &clean(token.expose()))?;
    Ok(path)
}

pub fn load_token(dir: &Path, agent: &str) -> io::Result<Secret> {
    let text = fs::read_to_string(dir.join(agent).join(TOKEN_FILE))?;
    Ok(Secret::new(clean(&text)))
}

/// Is a login for `agent` (as `harness.toml` names it) saved? The same
/// places the agents are started from; nothing is read beyond "is it there".
pub fn has_login(dir: &Path, agent: &str) -> bool {
    match agent {
        "claude" => dir.join("claude").join(TOKEN_FILE).is_file(),
        "codex" => dir.join("codex").join("auth.json").is_file(),
        "codex+deepseek" => {
            dir.join("deepseek").join(TOKEN_FILE).is_file()
                || std::env::var(crate::codex::DEEPSEEK_KEY_ENV)
                    .is_ok_and(|key| !key.trim().is_empty())
        }
        "antigravity" => dir.join("antigravity/.gemini/antigravity-cli").is_dir(),
        _ => false,
    }
}

/// The command that saves the login `agent` needs.
pub fn login_command(agent: &str) -> String {
    let name = if agent == "codex+deepseek" {
        "deepseek"
    } else {
        agent
    };
    format!("harness login {name}")
}

/// Where `harness secret set` keeps secrets for MCP servers, one file each.
const SECRETS_DIR: &str = "secrets";

/// Saves a secret an MCP server needs, for example an API key, readable only by Lisa.
pub fn save_secret(dir: &Path, name: &str, value: &Secret) -> io::Result<PathBuf> {
    let secrets = dir.join(SECRETS_DIR);
    fs::create_dir_all(&secrets)?;
    let path = secrets.join(name);
    write_private(&path, &clean(value.expose()))?;
    Ok(path)
}

pub fn load_secret(dir: &Path, name: &str) -> io::Result<Secret> {
    let text = fs::read_to_string(dir.join(SECRETS_DIR).join(name))?;
    Ok(Secret::new(clean(&text)))
}

/// The names of the saved secrets, sorted; never their values.
pub fn secret_names(dir: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    match fs::read_dir(dir.join(SECRETS_DIR)) {
        Ok(entries) => {
            for entry in entries {
                names.push(entry?.file_name().to_string_lossy().into_owned());
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    names.sort();
    Ok(names)
}

/// A token never contains spaces, but a copy from the terminal may add a line
/// break in the middle of a long token. Remove every kind of whitespace.
fn clean(token: &str) -> String {
    token.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Creates the file with permissions 600 (owner reads and writes, nobody else),
/// before any secret is written into it.
#[cfg(unix)]
pub(crate) fn write_private(path: &Path, text: &str) -> io::Result<()> {
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
pub(crate) fn write_private(path: &Path, text: &str) -> io::Result<()> {
    fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_is_seen_where_the_agents_look_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        assert!(!has_login(dir, "claude"));
        save_token(dir, "claude", &Secret::new("t")).unwrap();
        assert!(has_login(dir, "claude"));
        fs::create_dir_all(dir.join("codex")).unwrap();
        fs::write(dir.join("codex/auth.json"), "{}").unwrap();
        assert!(has_login(dir, "codex"));
        fs::create_dir_all(dir.join("antigravity/.gemini/antigravity-cli")).unwrap();
        assert!(has_login(dir, "antigravity"));
        save_token(dir, "deepseek", &Secret::new("k")).unwrap();
        assert!(has_login(dir, "codex+deepseek"));
        assert!(!has_login(dir, "gemini"));
        assert_eq!(login_command("codex+deepseek"), "harness login deepseek");
    }

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = save_token(dir.path(), "claude", &Secret::new(" token-\n1\n")).unwrap();
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
    fn secrets_are_saved_one_file_each_and_listed_by_name() {
        let dir = tempfile::tempdir().unwrap();
        assert!(secret_names(dir.path()).unwrap().is_empty());
        let path = save_secret(dir.path(), "context7", &Secret::new("ctx-1\n")).unwrap();
        save_secret(dir.path(), "github", &Secret::new("gh-2")).unwrap();
        assert_eq!(
            load_secret(dir.path(), "context7").unwrap().expose(),
            "ctx-1"
        );
        assert_eq!(secret_names(dir.path()).unwrap(), ["context7", "github"]);
        assert!(load_secret(dir.path(), "nope").is_err());

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
