//! `harness secret ...` and `harness mcp login`: secrets MCP servers need,
//! typed without echo and saved privately.

use std::io::{self, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use harness_agents::install::credentials::{self, Secret};
use harness_core::config::Config;
use harness_core::git::HARNESS_DIR;
use harness_core::mcp;

use crate::open_repo;

/// Reads one line from the terminal without echoing it.
pub(crate) fn read_hidden(prompt: &str) -> Result<Secret> {
    print!("{prompt}");
    io::stdout().flush()?;
    let line = harness_platform::terminal::read_line_hidden()?;
    Ok(Secret::new(line.trim()))
}

/// Saves a secret for MCP servers in `~/.harness/credentials/secrets/<name>`,
/// readable only by Lisa, typed without showing it on the screen.
pub(crate) fn set_secret(name: &str) -> Result<()> {
    if !mcp::is_simple_name(name) {
        bail!("use lowercase letters, digits, '-' and '_' for the name, for example `context7`");
    }
    let dir = credentials::default_dir().context("no home folder found")?;
    println!("Paste the secret for {name:?} and press Enter.");
    println!("It is not shown while you type: paste it once, then press Enter.");
    let value = read_hidden("Secret: ")?;
    if value.expose().is_empty() {
        bail!("no secret given; nothing was saved");
    }
    let path = credentials::save_secret(&dir, name, &value)?;
    println!(
        "Saved {} characters to {} (only you can read it).",
        value.expose().chars().count(),
        path.display()
    );
    println!("Use it in harness.toml as \"secret:{name}\".");
    Ok(())
}

/// `harness mcp login <name>`: signs in to an MCP server in the browser
/// (OAuth) and saves its token privately.
pub(crate) fn mcp_login(project: &Path, name: &str) -> Result<()> {
    use harness_agents::mcp::oauth;
    let repo = open_repo(project)?;
    let config = Config::load(&repo.root().join(HARNESS_DIR))?;
    let url = oauth::oauth_url(&config, name).map_err(anyhow::Error::msg)?;
    let dir = credentials::default_dir().context("no home folder found")?;
    println!("Signing in to {name} ({url}).");
    let open = |address: &str| {
        println!("Opening the browser. If it does not open, visit:\n{address}");
        if let Err(why) = oauth::open_in_browser(address) {
            println!("({why})");
        }
        Ok(())
    };
    let tools = oauth::Tools {
        curl: "curl".into(),
        open: &open,
        browser_limit: oauth::BROWSER_LIMIT,
    };
    oauth::login(&dir, name, &url, &tools).map_err(anyhow::Error::msg)?;
    println!(
        "Signed in. The tokens are in {} (only you can read them).",
        oauth::path(&dir, name).display()
    );
    Ok(())
}

/// `harness secret list`: prints the names of the saved secrets, never the values.
pub(crate) fn list_secrets() -> Result<()> {
    let dir = credentials::default_dir().context("no home folder found")?;
    let names = credentials::secret_names(&dir)?;
    if names.is_empty() {
        println!("No secrets saved. Add one with `harness secret set <name>`.");
    }
    for name in names {
        println!("{name}");
    }
    Ok(())
}
