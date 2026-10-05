//! `harness login <agent>`: saving each agent's login or API key in
//! `~/.harness/credentials/<agent>/`.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use harness_agents::credentials;

use crate::secrets::read_hidden;

/// `harness login <agent>`: saves the agent's login in
/// `~/.harness/credentials/<agent>/` (mode 600).
pub(crate) fn login(agent: &str) -> Result<()> {
    let dir = credentials::default_dir().context("no home folder found")?;
    match agent {
        "claude" => login_claude(&dir),
        "codex" => login_codex(&dir),
        "deepseek" => login_deepseek(&dir),
        "antigravity" => login_antigravity(&dir),
        other => {
            bail!("unknown agent {other:?}; use `claude`, `codex`, `deepseek` or `antigravity`")
        }
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

/// Saves the DeepSeek API key like the Claude token: in
/// `~/.harness/credentials/deepseek/`, readable only by Lisa. The key is typed
/// without showing it on the screen.
fn login_deepseek(dir: &Path) -> Result<()> {
    println!("Paste your DeepSeek API key (from platform.deepseek.com) and press Enter.");
    println!("It is not shown while you type: paste it once, then press Enter.");
    let key = read_hidden("API key: ")?;
    check_deepseek_key(key.expose())?;
    let path = credentials::save_token(dir, "deepseek", &key)?;
    println!(
        "Saved {} to {} (only you can read it).",
        masked(key.expose()),
        path.display()
    );
    Ok(())
}

/// A DeepSeek key is `sk-` and letters or digits, with no spaces. Pasting it
/// twice into the hidden prompt is easy, because nothing shows up.
fn check_deepseek_key(key: &str) -> Result<()> {
    let copies = key.matches("sk-").count();
    if copies > 1 {
        bail!("the key seems to be pasted {copies} times; run the command again and paste it once");
    }
    let body = key.strip_prefix("sk-").unwrap_or("");
    if body.len() < 16 || !body.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("this does not look like a DeepSeek API key (it starts with sk-); nothing was saved");
    }
    Ok(())
}

/// `sk-…1a2b (35 characters)`: enough to recognise the key, not to use it.
fn masked(key: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("sk-…{tail} ({} characters)", key.chars().count())
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
/// Lisa signs in with Google, then quits with `/quit`. The environment is
/// empty like for the agents, so the login goes to files, not the keyring.
fn login_antigravity(dir: &Path) -> Result<()> {
    let home = dir.join("antigravity");
    fs::create_dir_all(&home)?;
    harness_platform::private::restrict_dir(&home)?;
    println!("Starting `agy`. Sign in with Google, then type /quit to come back here.");
    let mut command = std::process::Command::new(harness_platform::program::resolve("agy"));
    command
        .env_clear()
        .envs(harness_platform::env::inherited_values());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deepseek_key_pasted_twice_is_refused() {
        let key = "sk-0123456789abcdef0123456789abcdef";
        assert!(check_deepseek_key(key).is_ok());
        let twice = format!("{key}{key}");
        let error = check_deepseek_key(&twice).unwrap_err().to_string();
        assert!(error.contains("2 times"), "{error}");
        assert!(check_deepseek_key("hello").is_err());
        assert!(check_deepseek_key("").is_err());
    }

    #[test]
    fn the_saved_key_is_shown_masked() {
        assert_eq!(
            masked("sk-0123456789abcdef0123456789abcdef"),
            "sk-…cdef (35 characters)"
        );
    }
}
