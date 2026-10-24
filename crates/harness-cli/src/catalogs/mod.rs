//! `harness marketplace ...` and `harness plugin ...`: plugin catalogs and
//! copying plugins from them into the project.
//!
//! Catalogs are the user's, for all projects: the list is in
//! `~/.harness/marketplaces.toml`, the copies in `~/.harness/marketplaces/`.
//! A plugin is always copied into the project (`.harness/plugins/<name>/`) and
//! committed, so the agents only ever see what is in the project's git.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use harness_core::config::AgentKind;
use harness_core::plugins::catalog::{Entry, Registry};
use harness_core::plugins::ops::{self, CatalogUpdate};

mod plugins;

pub use plugins::{plugin_add, plugin_list, plugin_remove, plugin_update, AddOptions};

/// `~/.harness`.
pub fn harness_home() -> Result<PathBuf> {
    harness_platform::home::harness_dir().context("no home folder found")
}

/// `harness marketplace add`: copies a plugin catalog (a git repository) into `~/.harness`.
pub fn marketplace_add(source: &str, name: Option<&str>) -> Result<()> {
    let home = harness_home()?;
    let catalog = match ops::add_catalog(&home, source, name) {
        Err(ops::OpsError::CatalogExists(name)) => bail!(
            "a catalog named {name:?} is already added; use `harness marketplace update` \
             or give this one another name with --name"
        ),
        other => other?,
    };
    println!(
        "Added catalog {}: {}.",
        catalog.name,
        count(&catalog.entries)
    );
    println!("See its plugins with `harness plugin list`.");
    Ok(())
}

/// "12 plugins: 10 for claude, 2 for codex".
fn count(entries: &[Entry]) -> String {
    let claude = entries
        .iter()
        .filter(|e| e.agent == AgentKind::Claude)
        .count();
    let codex = entries.len() - claude;
    format!(
        "{} plugins: {claude} for claude, {codex} for codex",
        entries.len()
    )
}

/// `harness marketplace list`: the catalogs, how many plugins each has and their commit.
pub fn marketplace_list() -> Result<()> {
    let home = harness_home()?;
    let catalogs = ops::catalogs(&home)?;
    if catalogs.is_empty() {
        println!("No catalogs yet. Add one with `harness marketplace add owner/repo`.");
    }
    for catalog in catalogs {
        let about = match &catalog.entries {
            Ok(entries) => count(entries),
            Err(e) => format!("cannot read: {e}"),
        };
        let commit = catalog
            .commit
            .map(|c| format!(" at {}", short(&c)))
            .unwrap_or_default();
        println!(
            "{}  {}{commit}  ({about})",
            catalog.name, catalog.config.source
        );
    }
    Ok(())
}

/// `harness marketplace update`: pulls one catalog, or all of them, again.
pub fn marketplace_update(name: Option<&str>) -> Result<()> {
    let home = harness_home()?;
    let names: Vec<String> = match name {
        Some(name) => vec![name.to_string()],
        None => Registry::load(&home)?.marketplaces.into_keys().collect(),
    };
    for name in names {
        match ops::update_catalog(&home, &name)? {
            CatalogUpdate::Local => println!("{name}: a local folder, always read as it is."),
            CatalogUpdate::Same(commit) => {
                println!("{name}: already up to date ({}).", short(&commit));
            }
            CatalogUpdate::Updated(commit) => println!("{name}: updated to {}.", short(&commit)),
        }
    }
    println!("Plugins already in projects are not changed; use `harness plugin update <name>`.");
    Ok(())
}

/// `harness marketplace remove`: forgets a catalog; plugins already in projects stay.
pub fn marketplace_remove(name: &str) -> Result<()> {
    let home = harness_home()?;
    ops::remove_catalog(&home, name)?;
    println!("Removed catalog {name}. Plugins already in projects stay there.");
    Ok(())
}

/// The first seven characters of a commit hash, as git shows it.
pub(super) fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}
