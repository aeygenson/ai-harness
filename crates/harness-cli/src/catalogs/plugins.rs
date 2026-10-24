//! `harness plugin ...`: copying plugins from the catalogs into the project.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};
use harness_core::config::{AgentKind, Config, CONFIG_FILE};
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::mcp::is_simple_name;
use harness_core::plugins::catalog::{Entry, Registry, Source};
use harness_core::plugins::install::Changes;
use harness_core::plugins::ops::{self, PluginChoice};
use harness_core::plugins::{Contents, PLUGINS_DIR};
use harness_core::task::handoff::Role;

use super::{harness_home, short};

/// `harness plugin list`: the plugins of all catalogs, marked when the project has them.
pub fn plugin_list(project: &Path, agent: Option<AgentKind>) -> Result<()> {
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
            Source::InCatalog(_) | Source::Git { .. } => String::new(),
        };
        // Writing into a `String` cannot fail, so `let _ =` ignores the `Result`.
        let _ = write!(
            out,
            "{}@{}  ({}){mark}{note}\n    {}\n",
            entry.name,
            entry.catalog,
            entry.agent,
            first_line(&entry.description, 100)
        );
    }
    // A long list is often piped to `head` or `less`; a closed pipe is not an error.
    let _ = std::io::Write::write_all(&mut std::io::stdout(), out.as_bytes());
    Ok(())
}

/// The options of `harness plugin add`.
#[derive(Debug)]
pub struct AddOptions {
    /// The agent the plugin is for, when the catalogs have it for more than one.
    pub agent: Option<AgentKind>,
    /// The role that gets the plugin; `None` adds it to the project only.
    pub role: Option<Role>,
    /// Allow the plugin's hooks (commands that run by themselves).
    pub allow_hooks: bool,
    /// Allow the plugin's own MCP or LSP servers.
    pub allow_mcp: bool,
}

/// `harness plugin add`: copies a plugin from a catalog into the project.
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
    let choice = PluginChoice {
        role: options.role,
        allow_hooks: options.allow_hooks,
        allow_mcp: options.allow_mcp,
    };
    let added = ops::add(&repo, &home, entry, choice)
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
        Some(role) => println!("The {role} role uses it now."),
        None => println!(
            "No role uses it yet: add \"{name}\" to `plugins = [...]` of a role on \"{}\" in {}.",
            entry.agent,
            relative(repo.root(), &config_path)
        ),
    }
    Ok(())
}

/// `harness plugin update`: takes the plugin's new version from its catalog; when that
/// fails, the old one is kept.
pub fn plugin_update(project: &Path, name: &str) -> Result<()> {
    let repo = open_repo(project)?;
    let home = harness_home()?;
    let Some(prepared) = ops::prepare_update(&repo, &home, name)? else {
        println!("Plugin {name} is already up to date.");
        return Ok(());
    };
    if let Err(e) = ops::apply_update(&repo, &prepared) {
        ops::discard(&prepared);
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

/// `harness plugin remove`: takes the plugin out of the project and every role.
pub fn plugin_remove(project: &Path, name: &str) -> Result<()> {
    let repo = open_repo(project)?;
    ops::remove(&repo, name)?;
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
