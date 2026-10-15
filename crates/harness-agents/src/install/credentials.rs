//! Login tokens, stored outside every project: `~/.harness/credentials/<agent>/`.
//!
//! A token is a password: it never goes into git, a handoff, a log or a debug print.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use harness_core::config::AgentKind;

use crate::adapters::dsh::Provider;

const TOKEN_FILE: &str = "oauth-token";

/// Lives in `harness_core`, because MCP server settings carry secrets too.
pub use harness_core::secret::Secret;

/// `~/.harness/credentials`.
pub fn default_dir() -> Option<PathBuf> {
    harness_platform::home::harness_dir().map(|dir| dir.join("credentials"))
}

/// Saves the token of `agent` (for example `claude`), readable only by Lisa.
pub fn save_token(dir: &Path, agent: &str, token: &Secret) -> io::Result<PathBuf> {
    let agent_dir = dir.join(agent);
    fs::create_dir_all(&agent_dir)?;
    let path = agent_dir.join(TOKEN_FILE);
    write_private(&path, &clean(token.expose()))?;
    Ok(path)
}

/// Reads the saved token of `agent` (for example `claude`), without any whitespace.
/// Fails when no token was saved.
pub fn load_token(dir: &Path, agent: &str) -> io::Result<Secret> {
    let text = fs::read_to_string(dir.join(agent).join(TOKEN_FILE))?;
    Ok(Secret::new(clean(&text)))
}

/// Is a login for `agent` (as `harness.toml` names it) there? The same
/// places the agents are started from: for dsh that is also a key in Lisa's
/// shell, and its key is read (a blank one does not count); for the others
/// nothing is read beyond "is it there".
pub fn has_login(dir: &Path, agent: AgentKind) -> bool {
    match agent {
        AgentKind::Claude => dir.join("claude").join(TOKEN_FILE).is_file(),
        AgentKind::Codex => dir.join("codex").join("auth.json").is_file(),
        AgentKind::Dsh => load_api_key(dir, Provider::DeepSeek).is_ok(),
        AgentKind::DshNvidia => load_api_key(dir, Provider::Nvidia).is_ok(),
        AgentKind::Antigravity => dir.join("antigravity/.gemini/antigravity-cli").is_dir(),
    }
}

/// The API key of a dsh `provider`: one in Lisa's shell wins; otherwise the
/// one «Sign in» saved. Fails when there is neither, or it is blank.
pub fn load_api_key(dir: &Path, provider: Provider) -> io::Result<Secret> {
    api_key_from(std::env::var(provider.key_env()).ok(), dir, provider)
}

/// [`load_api_key`] with the shell's value given, so tests do not depend on
/// Lisa's environment.
fn api_key_from(shell: Option<String>, dir: &Path, provider: Provider) -> io::Result<Secret> {
    if let Some(key) = shell_key(shell) {
        return Ok(key);
    }
    let saved = load_token(dir, provider.agent().login_name())?;
    if saved.expose().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the saved key is blank",
        ));
    }
    Ok(saved)
}

/// Whose login `agent` needs: `DeepSeek` Harness needs the `DeepSeek` key. A
/// catalog agent the harness cannot run keeps its own name.
pub fn login_name(agent: &str) -> &str {
    agent
        .parse::<AgentKind>()
        .map_or(agent, |kind| kind.login_name())
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

/// Reads a secret saved for an MCP server under `name`, without any whitespace.
/// Fails when no such secret was saved.
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

/// A key from Lisa's shell, cleaned like a saved one; `None` when blank.
fn shell_key(value: Option<String>) -> Option<Secret> {
    value
        .map(|key| clean(&key))
        .filter(|key| !key.is_empty())
        .map(Secret::new)
}

/// The tokens and API keys the agents use (Claude Code, `DeepSeek`, NVIDIA)
/// and MCP secrets, to hide them in text that leaves the computer: both the
/// saved key and one in Lisa's shell, since either may be in use. What cannot
/// be read is skipped.
pub fn saved_secrets(dir: &Path) -> Vec<Secret> {
    secrets_with(dir, &|name| std::env::var(name).ok())
}

/// [`saved_secrets`] with `shell` looking up a variable, so tests do not
/// depend on Lisa's environment.
fn secrets_with(dir: &Path, shell: &dyn Fn(&str) -> Option<String>) -> Vec<Secret> {
    let saved = ["claude", "deepseek", "nvidia"]
        .into_iter()
        .filter_map(|login| load_token(dir, login).ok());
    let shell = [Provider::DeepSeek, Provider::Nvidia]
        .into_iter()
        .filter_map(|provider| shell_key(shell(provider.key_env())));
    let tokens = saved.chain(shell);
    let secrets = secret_names(dir)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|name| load_secret(dir, &name).ok());
    tokens.chain(secrets).collect()
}

/// A token never contains spaces, but a copy from the terminal may add a line
/// break in the middle of a long token. Remove every kind of whitespace.
fn clean(token: &str) -> String {
    token.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Writes `text` into `path`, readable only by Lisa (see `harness_platform::private`).
pub(crate) fn write_private(path: &Path, text: &str) -> io::Result<()> {
    harness_platform::private::write(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_is_seen_where_the_agents_look_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        assert!(!has_login(dir, AgentKind::Claude));
        save_token(dir, "claude", &Secret::new("t")).unwrap();
        assert!(has_login(dir, AgentKind::Claude));
        fs::create_dir_all(dir.join("codex")).unwrap();
        fs::write(dir.join("codex/auth.json"), "{}").unwrap();
        assert!(has_login(dir, AgentKind::Codex));
        fs::create_dir_all(dir.join("antigravity/.gemini/antigravity-cli")).unwrap();
        assert!(has_login(dir, AgentKind::Antigravity));
        save_token(dir, "deepseek", &Secret::new("k")).unwrap();
        assert!(has_login(dir, AgentKind::Dsh));
        assert_eq!(login_name("dsh"), "deepseek");
        save_token(dir, "nvidia", &Secret::new("nvapi-k")).unwrap();
        assert!(has_login(dir, AgentKind::DshNvidia));
        assert_eq!(login_name("dsh+nvidia"), "nvidia");
    }

    #[test]
    fn a_key_in_the_shell_wins_and_a_blank_one_falls_back_to_the_saved_one() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let nvidia = Provider::Nvidia;
        api_key_from(None, dir, nvidia).unwrap_err();
        save_token(dir, "nvidia", &Secret::new("nvapi-saved")).unwrap();
        let key = |shell: Option<&str>| {
            api_key_from(shell.map(str::to_string), dir, nvidia)
                .unwrap()
                .expose()
                .to_string()
        };
        assert_eq!(key(None), "nvapi-saved");
        assert_eq!(key(Some(" nvapi-shell \n")), "nvapi-shell");
        assert_eq!(key(Some("  ")), "nvapi-saved");
        // A blank saved key is no key.
        save_token(dir, "nvidia", &Secret::new("")).unwrap();
        api_key_from(None, dir, nvidia).unwrap_err();
        // The DeepSeek key is looked up in its own folder.
        api_key_from(None, dir, Provider::DeepSeek).unwrap_err();
    }

    #[test]
    fn every_saved_key_is_hidden() {
        let dir = tempfile::tempdir().unwrap();
        save_token(dir.path(), "nvidia", &Secret::new("nvapi-saved")).unwrap();
        save_token(dir.path(), "deepseek", &Secret::new("sk-saved")).unwrap();
        let hidden: Vec<String> = saved_secrets(dir.path())
            .iter()
            .map(|s| s.expose().to_string())
            .collect();
        assert!(hidden.contains(&"nvapi-saved".to_string()), "{hidden:?}");
        assert!(hidden.contains(&"sk-saved".to_string()), "{hidden:?}");

        // A key in the shell is hidden too, cleaned like a saved one.
        let shell = |name: &str| (name == "NVIDIA_API_KEY").then(|| " nvapi-sh\nell ".to_string());
        let hidden: Vec<String> = secrets_with(dir.path(), &shell)
            .iter()
            .map(|s| s.expose().to_string())
            .collect();
        assert!(hidden.contains(&"nvapi-shell".to_string()), "{hidden:?}");
        assert!(hidden.contains(&"nvapi-saved".to_string()), "{hidden:?}");
    }

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = save_token(dir.path(), "claude", &Secret::new(" token-\n1\n")).unwrap();
        assert_eq!(
            load_token(dir.path(), "claude").unwrap().expose(),
            "token-1"
        );

        assert!(harness_platform::private::is_private(&path).unwrap());
    }

    #[test]
    fn secrets_are_saved_one_file_each_and_listed_by_name() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(secret_names(dir.path()).unwrap(), Vec::<String>::new());
        let path = save_secret(dir.path(), "context7", &Secret::new("ctx-1\n")).unwrap();
        save_secret(dir.path(), "github", &Secret::new("gh-2")).unwrap();
        assert_eq!(
            load_secret(dir.path(), "context7").unwrap().expose(),
            "ctx-1"
        );
        assert_eq!(secret_names(dir.path()).unwrap(), ["context7", "github"]);
        load_secret(dir.path(), "nope").unwrap_err();

        assert!(harness_platform::private::is_private(&path).unwrap());
    }

    #[test]
    fn a_missing_token_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        load_token(dir.path(), "claude").unwrap_err();
    }
}
