//! `harness marketplace ...` and `harness plugin ...`: plugin catalogs and
//! copying plugins from them into the project.
//!
//! Catalogs are Lisa's, for all projects: the list is in
//! `~/.harness/marketplaces.toml`, the copies in `~/.harness/marketplaces/`.
//! A plugin is always copied into the project (`.harness/plugins/<name>/`) and
//! committed, so the agents only ever see what is in the project's git.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use harness_core::catalog::{Entry, Registry, Source};
use harness_core::config::{Config, CONFIG_FILE};
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::handoff::Role;
use harness_core::mcp::is_simple_name;
use harness_core::plugin_install::Changes;
use harness_core::plugin_ops::{self, CatalogUpdate};
use harness_core::plugins::{self, Contents, PLUGINS_DIR};

/// `~/.harness`.
pub fn harness_home() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".harness"))
}

pub fn marketplace_add(source: &str, name: Option<&str>) -> Result<()> {
    let home = harness_home()?;
    let catalog = match plugin_ops::add_catalog(&home, source, name) {
        Err(plugin_ops::OpsError::CatalogExists(name)) => bail!(
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
        .filter(|e| e.agent == plugins::CLAUDE)
        .count();
    let codex = entries.len() - claude;
    format!(
        "{} plugins: {claude} for claude, {codex} for codex",
        entries.len()
    )
}

pub fn marketplace_list() -> Result<()> {
    let home = harness_home()?;
    let catalogs = plugin_ops::catalogs(&home)?;
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

pub fn marketplace_update(name: Option<&str>) -> Result<()> {
    let home = harness_home()?;
    let names: Vec<String> = match name {
        Some(name) => vec![name.to_string()],
        None => Registry::load(&home)?.marketplaces.into_keys().collect(),
    };
    for name in names {
        match plugin_ops::update_catalog(&home, &name)? {
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

pub fn marketplace_remove(name: &str) -> Result<()> {
    let home = harness_home()?;
    plugin_ops::remove_catalog(&home, name)?;
    println!("Removed catalog {name}. Plugins already in projects stay there.");
    Ok(())
}

pub fn plugin_list(project: &Path, agent: Option<&str>) -> Result<()> {
    let home = harness_home()?;
    let registry = Registry::load(&home)?;
    let (entries, errors) = registry.entries(&home);
    for error in errors {
        eprintln!("warning: {error}");
    }
    if registry.marketplaces.is_empty() {
        println!("No catalogs yet. Add one with `harness marketplace add owner/repo`.");
        return Ok(());
    }
    // Plugins this project already has, by `<catalog>/<plugin>`.
    let added: Vec<String> = Repo::open(project)
        .ok()
        .and_then(|repo| Config::load(&repo.root().join(HARNESS_DIR)).ok())
        .map(|config| {
            config
                .plugins
                .values()
                .filter_map(|p| p.source.clone())
                .collect()
        })
        .unwrap_or_default();
    let mut out = String::new();
    for entry in entries
        .iter()
        .filter(|e| agent.is_none_or(|agent| e.agent == agent))
    {
        let mark = if added.contains(&entry.id()) {
            "  [in this project]"
        } else {
            ""
        };
        let note = match &entry.source {
            Source::Unsupported(kind) => format!("  (from {kind}: not supported yet)"),
            _ => String::new(),
        };
        out.push_str(&format!(
            "{}@{}  ({}){mark}{note}\n    {}\n",
            entry.name,
            entry.catalog,
            entry.agent,
            first_line(&entry.description, 100)
        ));
    }
    // A long list is often piped to `head` or `less`; a closed pipe is not an error.
    let _ = std::io::Write::write_all(&mut std::io::stdout(), out.as_bytes());
    Ok(())
}

/// The options of `harness plugin add`.
pub struct AddOptions<'a> {
    pub agent: Option<&'a str>,
    pub role: Option<Role>,
    pub allow_hooks: bool,
    pub allow_mcp: bool,
}

pub fn plugin_add(project: &Path, wanted: &str, options: &AddOptions) -> Result<()> {
    let (name, catalog_name) = match wanted.split_once('@') {
        Some((name, catalog)) => (name, Some(catalog)),
        None => (wanted, None),
    };
    if !is_simple_name(name) {
        bail!(
            "plugin name {name:?} cannot be used in a project; the harness needs lowercase \
             letters, digits, '-' and '_'"
        );
    }
    let repo = open_repo(project)?;
    let config_path = repo.root().join(HARNESS_DIR).join(CONFIG_FILE);
    if !config_path.is_file() {
        bail!("cannot read {}; run `harness init`", config_path.display());
    }

    let home = harness_home()?;
    let registry = Registry::load(&home)?;
    let (entries, _) = registry.entries(&home);
    let found: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.name == name)
        .filter(|e| catalog_name.is_none_or(|c| e.catalog == c))
        .filter(|e| options.agent.is_none_or(|a| e.agent == a))
        .collect();
    let entry = match found.as_slice() {
        [] => bail!("no plugin {wanted:?} in the catalogs; see `harness plugin list`"),
        [one] => *one,
        many => bail!(
            "{name:?} is in more than one place; say which one:\n  {}",
            many.iter()
                .map(|e| format!(
                    "harness plugin add {}@{} --agent {}",
                    e.name, e.catalog, e.agent
                ))
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    };

    let target = repo.root().join(PLUGINS_DIR).join(name);
    if target.exists() {
        bail!(
            "{} already exists; use `harness plugin update {name}`",
            target.display()
        );
    }
    let added = plugin_ops::add(
        &repo,
        &home,
        entry,
        options.role,
        options.allow_hooks,
        options.allow_mcp,
    )
    .with_context(|| format!("plugin {name} was not added"))?;
    let (contents, commit) = (added.contents, added.commit);

    println!(
        "Added plugin {name} ({}) from {}{} into {}.",
        entry.agent,
        entry.id(),
        commit
            .as_deref()
            .map(|c| format!(" at {}", short(c)))
            .unwrap_or_default(),
        relative(repo.root(), &target)
    );
    warn_about(name, contents, options.allow_hooks, options.allow_mcp);
    match options.role {
        Some(role) => println!("The {role:?} role uses it now."),
        None => println!(
            "No role uses it yet: add \"{name}\" to `plugins = [...]` of a role on \"{}\" in {}.",
            entry.agent,
            relative(repo.root(), &config_path)
        ),
    }
    Ok(())
}

pub fn plugin_update(project: &Path, name: &str) -> Result<()> {
    let repo = open_repo(project)?;
    let home = harness_home()?;
    let Some(prepared) = plugin_ops::prepare_update(&repo, &home, name)? else {
        println!("Plugin {name} is already up to date.");
        return Ok(());
    };
    if let Err(e) = plugin_ops::apply_update(&repo, &prepared) {
        plugin_ops::discard(&prepared);
        return Err(anyhow::Error::new(e).context(format!(
            "the new version of {name} was not taken; the old one is kept"
        )));
    }
    let config = Config::load(&repo.root().join(HARNESS_DIR))?;
    let plugin = &config.plugins[name];
    println!(
        "Updated plugin {name}{}:",
        prepared
            .commit
            .as_deref()
            .map(|c| format!(" to {}", short(c)))
            .unwrap_or_default()
    );
    print_changes(&prepared.changes);
    warn_about(
        name,
        prepared.contents,
        plugin.allow_hooks,
        plugin.allow_mcp,
    );
    Ok(())
}

pub fn plugin_remove(project: &Path, name: &str) -> Result<()> {
    let repo = open_repo(project)?;
    plugin_ops::remove(&repo, name)?;
    println!("Removed plugin {name} from the project and from every role.");
    Ok(())
}

fn warn_about(name: &str, contents: Contents, allow_hooks: bool, allow_mcp: bool) {
    if contents.hooks && !allow_hooks {
        println!(
            "Note: {name} has hooks (commands that run by themselves). A role can use it only \
             after you add `allow_hooks = true` to [plugins.{name}]."
        );
    }
    if contents.servers && !allow_mcp {
        println!(
            "Note: {name} starts its own MCP or LSP servers. A role can use it only after you \
             add `allow_mcp = true` to [plugins.{name}]."
        );
    }
}

fn print_changes(changes: &Changes) {
    for (sign, files) in [
        ("+", &changes.added),
        ("~", &changes.changed),
        ("-", &changes.removed),
    ] {
        for file in files {
            println!("  {sign} {file}");
        }
    }
}

fn open_repo(project: &Path) -> Result<Repo> {
    Repo::open(project).context("the project must be a git repository (run `git init`)")
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let cut: String = line.chars().take(max - 3).collect();
        format!("{cut}...")
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_descriptions_are_cut() {
        assert_eq!(first_line("short\nsecond", 10), "short");
        assert_eq!(first_line("abcdefghijkl", 8), "abcde...");
    }
}
