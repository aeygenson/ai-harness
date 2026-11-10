//! `harness login <agent>`: saving each agent's login or API key in
//! `~/.harness/credentials/<agent>/`.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use harness_agents::install::credentials;
use harness_core::config::AgentKind;

use crate::secrets::read_hidden;

/// `harness login <agent>`: saves the agent's login in
/// `~/.harness/credentials/<agent>/` (mode 600).
pub(crate) fn login(agent: &str) -> Result<()> {
    let dir = credentials::default_dir().context("no home folder found")?;
    match agent {
        "claude" => login_claude(&dir),
        "codex" => login_codex(&dir),
        "deepseek" => login_api_key(&dir, &DEEPSEEK),
        "nvidia" => login_api_key(&dir, &NVIDIA),
        "antigravity" => login_antigravity(&dir),
        "cursor" => login_cursor(&dir, &harness_agents::adapters::cursor::program()),
        "gemini" => login_gemini(&dir, Path::new("gemini")),
        other => bail!(
            "unknown agent {other:?}; use `claude`, `codex`, `deepseek`, `nvidia`, \
             `antigravity`, `cursor` or `gemini`"
        ),
    }
}

/// Runs `claude setup-token` (it opens the browser and prints a long-lived
/// token), then asks for that token and saves it.
fn login_claude(dir: &Path) -> Result<()> {
    println!("Starting `claude setup-token`; finish the login in your browser.");
    let status = std::process::Command::new(harness_platform::program::resolve("claude"))
        .arg("setup-token")
        .status()
        .context("cannot start `claude`; is Claude Code installed?")?;
    if !status.success() {
        bail!("`claude setup-token` failed ({status})");
    }
    println!();
    println!("Copy the token it printed above, paste it here and press Enter.");
    println!("It is not shown while you type: paste it once, then press Enter.");
    let token = read_hidden("Token: ")?;
    if token.expose().is_empty() {
        bail!("no token given");
    }
    let path = credentials::save_token(dir, "claude", &token)?;
    println!("Saved to {} (only you can read it).", path.display());
    Ok(())
}

/// What an API key looks like, to catch a wrong paste before it is saved.
struct KeyShape {
    /// The company, as the user knows it.
    vendor: &'static str,
    /// The folder in `~/.harness/credentials/`.
    login: &'static str,
    /// Where the user gets the key.
    site: &'static str,
    /// How every key starts.
    prefix: &'static str,
    /// Characters allowed after the prefix besides letters and digits.
    extra: &'static [char],
}

/// `DeepSeek`: `sk-` and letters or digits.
const DEEPSEEK: KeyShape = KeyShape {
    vendor: "DeepSeek",
    login: AgentKind::Dsh.login_name(),
    site: "platform.deepseek.com",
    prefix: "sk-",
    extra: &[],
};

/// NVIDIA (build.nvidia.com): `nvapi-` and letters, digits, `-` or `_`.
const NVIDIA: KeyShape = KeyShape {
    vendor: "NVIDIA",
    login: AgentKind::DshNvidia.login_name(),
    site: "build.nvidia.com, «Get API Key»",
    prefix: "nvapi-",
    extra: &['-', '_'],
};

/// Saves an API key like the Claude token: in
/// `~/.harness/credentials/<login>/`, readable only by the user. The key is typed
/// without showing it on the screen.
fn login_api_key(dir: &Path, shape: &KeyShape) -> Result<()> {
    println!(
        "Paste your {} API key (from {}) and press Enter.",
        shape.vendor, shape.site
    );
    println!("It is not shown while you type: paste it once, then press Enter.");
    let key = read_hidden("API key: ")?;
    check_key(key.expose(), shape)?;
    let path = credentials::save_token(dir, shape.login, &key)?;
    println!(
        "Saved {} to {} (only you can read it).",
        masked(key.expose(), shape.prefix),
        path.display()
    );
    Ok(())
}

/// A key is the prefix and at least 16 allowed characters, with no spaces.
/// Pasting it twice into the hidden prompt is easy, because nothing shows up.
fn check_key(key: &str, shape: &KeyShape) -> Result<()> {
    let copies = key.matches(shape.prefix).count();
    if copies > 1 {
        bail!("the key seems to be pasted {copies} times; run the command again and paste it once");
    }
    let body = key.strip_prefix(shape.prefix).unwrap_or("");
    let allowed = |c: char| c.is_ascii_alphanumeric() || shape.extra.contains(&c);
    if body.len() < 16 || !body.chars().all(allowed) {
        bail!(
            "this does not look like a {} API key (it starts with {}); nothing was saved",
            shape.vendor,
            shape.prefix
        );
    }
    Ok(())
}

/// `sk-…1a2b (35 characters)`: enough to recognise the key, not to use it.
fn masked(key: &str, prefix: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}…{tail} ({} characters)", key.chars().count())
}

/// Runs `codex login` with its home in our credentials folder, so the login is
/// saved there and not in `~/.codex`.
fn login_codex(dir: &Path) -> Result<()> {
    let home = dir.join("codex");
    fs::create_dir_all(&home)?;
    println!("Starting `codex login`; finish the login in your browser.");
    let status = std::process::Command::new(harness_platform::program::resolve("codex"))
        .arg("login")
        .env("CODEX_HOME", &home)
        .status()
        .context("cannot start `codex`; is Codex CLI installed?")?;
    if !status.success() {
        bail!("`codex login` failed ({status})");
    }
    let auth = home.join("auth.json");
    if !auth.exists() {
        bail!(
            "`codex login` finished but {} was not created",
            auth.display()
        );
    }
    harness_platform::private::restrict_file(&auth)?;
    println!("Saved to {} (only you can read it).", auth.display());
    Ok(())
}

/// Starts `agy` with `HOME` set to our credentials folder, so the login is
/// saved there and not in `~/.gemini`. `agy` has no separate login command:
/// The user signs in with Google, then quits with `/quit`. The environment is
/// empty like for the agents, so the login goes to files, not the keyring.
fn login_antigravity(dir: &Path) -> Result<()> {
    let home = dir.join("antigravity");
    fs::create_dir_all(&home)?;
    harness_platform::private::restrict_dir(&home)?;
    println!("Starting `agy`. Sign in with Google, then type /quit to come back here.");
    let mut command = std::process::Command::new(harness_platform::program::resolve("agy"));
    // In its own folder, so no project's `.gemini/settings.json` or MCP
    // servers are started while signing in.
    command
        .env_clear()
        .envs(harness_platform::env::inherited_values())
        .current_dir(&home);
    harness_platform::home::set_for(&mut command, &home);
    let status = command
        .status()
        .context("cannot start `agy`; is Antigravity CLI installed?")?;
    if !status.success() {
        bail!("`agy` failed ({status})");
    }
    if !home.join(".gemini/antigravity-cli").is_dir() {
        bail!(
            "`agy` finished but did not save a login in {}",
            home.display()
        );
    }
    println!("Saved in {} (only you can read it).", home.display());
    Ok(())
}

/// Runs `gemini` with its own `HOME` in `credentials/gemini`, where the
/// user signs in with Google; roles later get a copy of that login (see
/// `harness_agents::adapters::gemini`), never the user's own Gemini setup.
fn login_gemini(dir: &Path, program: &Path) -> Result<()> {
    use harness_agents::adapters::gemini::{LOGIN_FILE, SETTINGS_DIR};
    let home = dir.join("gemini");
    fs::create_dir_all(&home)?;
    harness_platform::private::restrict_dir(&home)?;
    println!("Starting `gemini`. Choose «Sign in with Google», then type /quit to come back here.");
    let mut command = std::process::Command::new(harness_platform::program::resolve(program));
    command
        .env_clear()
        .envs(harness_platform::env::inherited_values());
    harness_platform::home::set_for(&mut command, &home);
    let status = command
        .status()
        .context("cannot start `gemini`; is Gemini CLI installed?")?;
    let login = home.join(SETTINGS_DIR).join(LOGIN_FILE);
    // Quitting with Ctrl+C after signing in ends `gemini` with an error code,
    // yet the login is saved: the file decides.
    if !status.success() && !login.is_file() {
        bail!("`gemini` failed ({status})");
    }
    if !login.is_file() {
        bail!(
            "`gemini` finished but did not save a Google login in {}",
            home.display()
        );
    }
    harness_platform::private::restrict_file(&login)?;
    println!("Saved in {} (only you can read it).", home.display());
    Ok(())
}

/// Runs Cursor's own `agent login` (in the browser), then `agent status` to
/// check it. Cursor saves the login itself (on a Mac in the Keychain), so the
/// harness never sees a key; it only notes in `credentials/cursor/` that the
/// user signed in, so the Agents tab can show it.
fn login_cursor(dir: &Path, program: &Path) -> Result<()> {
    println!("Starting `agent login`; finish the login in your browser.");
    for step in ["login", "status"] {
        let status = std::process::Command::new(harness_platform::program::resolve(program))
            .arg(step)
            .status()
            .context("cannot start `agent`; is Cursor CLI installed?")?;
        if !status.success() {
            bail!("`agent {step}` failed ({status}); nothing was saved");
        }
    }
    let note = dir.join("cursor");
    fs::create_dir_all(&note)?;
    fs::write(
        note.join(credentials::CURSOR_SIGNED_IN),
        "Signed in with `agent login`; Cursor keeps the login itself.\n",
    )?;
    println!("Signed in to Cursor.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_counts_as_signed_in_only_after_login_and_status_succeed() {
        let creds = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let failing = harness_fake::install(bin.path(), "agent", "when-arg 1 status exit 1\n");
        assert!(login_cursor(creds.path(), &failing).is_err());
        assert!(!credentials::has_login(creds.path(), AgentKind::Cursor));

        let working = harness_fake::install(bin.path(), "agent", "print ok\n");
        login_cursor(creds.path(), &working).unwrap();
        assert!(credentials::has_login(creds.path(), AgentKind::Cursor));
    }

    #[test]
    fn a_deepseek_key_pasted_twice_is_refused() {
        let key = "sk-0123456789abcdef0123456789abcdef";
        check_key(key, &DEEPSEEK).unwrap();
        let twice = format!("{key}{key}");
        let error = check_key(&twice, &DEEPSEEK).unwrap_err().to_string();
        assert!(error.contains("2 times"), "{error}");
        assert!(check_key("hello", &DEEPSEEK).is_err());
        assert!(check_key("", &DEEPSEEK).is_err());
        // `-` belongs to NVIDIA's keys, not to DeepSeek's.
        assert!(check_key("sk-0123456789-abcdef0123456789", &DEEPSEEK).is_err());
    }

    #[test]
    fn an_nvidia_key_is_checked_by_its_own_shape() {
        let key = "nvapi-AbC_123-xyz0123456789abcdefGHIJ";
        check_key(key, &NVIDIA).unwrap();
        let error = check_key("sk-0123456789abcdef0123456789abcdef", &NVIDIA)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("NVIDIA") && error.contains("nvapi-"),
            "{error}"
        );
        let twice = format!("{key}{key}");
        assert!(check_key(&twice, &NVIDIA)
            .unwrap_err()
            .to_string()
            .contains("2 times"));
    }

    #[test]
    fn the_saved_key_is_shown_masked() {
        assert_eq!(
            masked("sk-0123456789abcdef0123456789abcdef", "sk-"),
            "sk-…cdef (35 characters)"
        );
    }

    #[test]
    fn gemini_counts_as_signed_in_only_when_it_saved_a_google_login() {
        let creds = tempfile::tempdir().unwrap();
        let bin = creds.path().join("bin");
        let quits = harness_fake::install(&bin, "gemini-quits", "exit 0\n");
        assert!(login_gemini(creds.path(), &quits).is_err());
        assert!(!credentials::has_login(creds.path(), AgentKind::Gemini));

        let signs_in = harness_fake::install(
            &bin,
            "gemini-signs-in",
            "save-text ${HOME}/.gemini/oauth_creds.json {}\n",
        );
        login_gemini(creds.path(), &signs_in).unwrap();

        assert!(credentials::has_login(creds.path(), AgentKind::Gemini));
    }
}
