//! `harness marketplace ...` and `harness plugin ...`: plugin catalogs and
//! copying plugins from them into the project.
//!
//! Catalogs are Lisa's, for all projects: the list is in
//! `~/.harness/marketplaces.toml`, the copies in `~/.harness/marketplaces/`.
//! A plugin is always copied into the project (`.harness/plugins/<name>/`) and
//! committed, so the agents only ever see what is in the project's git.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use harness_core::catalog::{self, Catalog, CatalogConfig, Entry, Registry, Source};
use harness_core::config::{Config, CONFIG_FILE};
use harness_core::config_edit::{self, NewPlugin};
use harness_core::git::{self, Repo, HARNESS_DIR};
use harness_core::handoff::Role;
use harness_core::mcp::is_simple_name;
use harness_core::plugin_install::{self, Changes};
use harness_core::plugin_ops;
use harness_core::plugins::{self, Contents, Plugins, PLUGINS_DIR};

/// A catalog being added is downloaded here first, before its name is known.
const ADDING_DIR: &str = ".adding";
/// Plugins that live in other repositories are downloaded here.
const DOWNLOADS_DIR: &str = ".downloads";

/// `~/.harness`.
pub fn harness_home() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".harness"))
}

pub fn marketplace_add(source: &str, name: Option<&str>) -> Result<()> {
    let home = harness_home()?;
    let mut registry = Registry::load(&home)?;
    let catalogs = home.join(catalog::CATALOGS_DIR);
    let local = Path::new(source);
    let (config, catalog) = if local.is_dir() {
        let path = local
            .canonicalize()
            .with_context(|| format!("cannot open {source}"))?;
        let catalog = Catalog::read(&path, name)?;
        let config = CatalogConfig {
            source: path.display().to_string(),
            local: true,
        };
        (config, catalog)
    } else {
        let url = clone_url(source)?;
        let adding = catalogs.join(ADDING_DIR);
        let _ = fs::remove_dir_all(&adding);
        git::fetch(&url, None, &adding)
            .with_context(|| format!("cannot download the catalog {url}"))?;
        let catalog = match Catalog::read(&adding, name) {
            Ok(catalog) => catalog,
            Err(e) => {
                let _ = fs::remove_dir_all(&adding);
                return Err(e.into());
            }
        };
        if !registry.marketplaces.contains_key(&catalog.name) {
            let target = catalogs.join(&catalog.name);
            let _ = fs::remove_dir_all(&target);
            fs::rename(&adding, &target)?;
        } else {
            let _ = fs::remove_dir_all(&adding);
        }
        let config = CatalogConfig {
            source: url,
            local: false,
        };
        (config, catalog)
    };
    if registry.marketplaces.contains_key(&catalog.name) {
        bail!(
            "a catalog named {:?} is already added; use `harness marketplace update` \
             or give this one another name with --name",
            catalog.name
        );
    }
    registry.marketplaces.insert(catalog.name.clone(), config);
    registry.save(&home)?;
    println!(
        "Added catalog {}: {}.",
        catalog.name,
        count(&catalog.entries)
    );
    println!("See its plugins with `harness plugin list`.");
    Ok(())
}

/// `owner/repo` means GitHub; anything that looks like a git address is used as it is.
fn clone_url(source: &str) -> Result<String> {
    if source.contains("://") || source.starts_with("git@") || source.ends_with(".git") {
        return Ok(source.to_string());
    }
    let parts: Vec<&str> = source.split('/').collect();
    let simple = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if parts.len() == 2 && parts.iter().all(|part| simple(part)) {
        return Ok(catalog::github_url(source));
    }
    bail!("{source:?} is not a folder, `owner/repo` on GitHub or a git address")
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
    let registry = Registry::load(&home)?;
    if registry.marketplaces.is_empty() {
        println!("No catalogs yet. Add one with `harness marketplace add owner/repo`.");
    }
    for (name, config) in &registry.marketplaces {
        let dir = registry.dir(&home, name);
        let about = match Catalog::read(&dir, Some(name)) {
            Ok(catalog) => count(&catalog.entries),
            Err(e) => format!("cannot read: {e}"),
        };
        let commit = git::head_commit(&dir)
            .map(|c| format!(" at {}", short(&c)))
            .unwrap_or_default();
        println!("{name}  {}{commit}  ({about})", config.source);
    }
    Ok(())
}

pub fn marketplace_update(name: Option<&str>) -> Result<()> {
    let home = harness_home()?;
    let registry = Registry::load(&home)?;
    let names: Vec<&String> = match name {
        Some(name) => vec![
            registry
                .marketplaces
                .get_key_value(name)
                .with_context(|| format!("no catalog named {name:?}"))?
                .0,
        ],
        None => registry.marketplaces.keys().collect(),
    };
    for name in names {
        let config = &registry.marketplaces[name];
        if config.local {
            println!("{name}: a local folder, always read as it is.");
            continue;
        }
        let dir = registry.dir(&home, name);
        let before = git::head_commit(&dir);
        let after = git::fetch(&config.source, None, &dir)
            .with_context(|| format!("cannot update catalog {name}"))?;
        if before.as_deref() == Some(after.as_str()) {
            println!("{name}: already up to date ({}).", short(&after));
        } else {
            println!("{name}: updated to {}.", short(&after));
        }
    }
    println!("Plugins already in projects are not changed; use `harness plugin update <name>`.");
    Ok(())
}

pub fn marketplace_remove(name: &str) -> Result<()> {
    let home = harness_home()?;
    let mut registry = Registry::load(&home)?;
    let Some(config) = registry.marketplaces.remove(name) else {
        bail!("no catalog named {name:?}");
    };
    let catalogs = home.join(catalog::CATALOGS_DIR);
    if !config.local {
        let _ = fs::remove_dir_all(catalogs.join(name));
    }
    let _ = fs::remove_dir_all(catalogs.join(DOWNLOADS_DIR).join(name));
    registry.save(&home)?;
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
    let harness_dir = repo.root().join(HARNESS_DIR);
    let config_path = harness_dir.join(CONFIG_FILE);
    let text = fs::read_to_string(&config_path)
        .with_context(|| format!("cannot read {}; run `harness init`", config_path.display()))?;

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
    let staged = staging(&target);
    let (contents, commit) = fetch_and_stage(&home, &registry, entry, &staged)?;
    let new_text = config_edit::add_plugin(
        &text,
        &NewPlugin {
            name,
            agent: entry.agent,
            source: &entry.id(),
            commit: commit.as_deref(),
            allow_hooks: options.allow_hooks,
            allow_mcp: options.allow_mcp,
        },
        options.role,
    );
    let new_text = match new_text {
        Ok(text) => text,
        Err(e) => {
            let _ = fs::remove_dir_all(&staged);
            return Err(e.into());
        }
    };
    plugin_install::put_in_place(&staged, &target)?;
    fs::write(&config_path, &new_text)?;
    if let Err(e) = Plugins::load(repo.root(), &Config::parse(&new_text)?) {
        let _ = fs::remove_dir_all(&target);
        fs::write(&config_path, &text)?;
        return Err(anyhow::Error::new(e).context(format!("plugin {name} was not added")));
    }
    repo.commit_paths(
        &[&target, &config_path],
        &format!("harness: add plugin {name} from {}", entry.id()),
    )?;

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
    let harness_dir = repo.root().join(HARNESS_DIR);
    let config_path = harness_dir.join(CONFIG_FILE);
    let text = fs::read_to_string(&config_path)?;
    let config = Config::parse(&text)?;
    let Some(plugin) = config.plugins.get(name) else {
        bail!("harness.toml has no [plugins.{name}]");
    };
    let Some((catalog_name, entry_name)) = plugin.source.as_deref().and_then(|s| s.split_once('/'))
    else {
        bail!("plugin {name} was not added from a catalog (it has no `source`), so it cannot be updated");
    };
    let home = harness_home()?;
    let registry = Registry::load(&home)?;
    if !registry.marketplaces.contains_key(catalog_name) {
        bail!("plugin {name} came from catalog {catalog_name:?}, which is not added any more");
    }
    let catalog = Catalog::read(&registry.dir(&home, catalog_name), Some(catalog_name))?;
    let entry = catalog
        .entries
        .iter()
        .find(|e| e.name == entry_name && e.agent == plugin.agent)
        .with_context(|| format!("catalog {catalog_name} no longer lists {entry_name}"))?;

    let target = repo.root().join(plugins::relative_path(name, plugin));
    let staged = staging(&target);
    let (contents, commit) = fetch_and_stage(&home, &registry, entry, &staged)?;
    let found = plugin_install::changes(&target, &staged)?;
    if found.is_empty() {
        let _ = fs::remove_dir_all(&staged);
        println!("Plugin {name} is already up to date.");
        return Ok(());
    }

    let old = target.with_file_name(format!(".{name}.old"));
    let _ = fs::remove_dir_all(&old);
    fs::rename(&target, &old)?;
    plugin_install::put_in_place(&staged, &target)?;
    let new_text = match &commit {
        Some(commit) => config_edit::set_plugin_commit(&text, name, commit)?,
        None => text.clone(),
    };
    fs::write(&config_path, &new_text)?;
    if let Err(e) = Plugins::load(repo.root(), &Config::parse(&new_text)?) {
        let _ = fs::remove_dir_all(&target);
        fs::rename(&old, &target)?;
        fs::write(&config_path, &text)?;
        return Err(anyhow::Error::new(e).context(format!(
            "the new version of {name} was not taken; the old one is kept"
        )));
    }
    let _ = fs::remove_dir_all(&old);
    repo.commit_paths(
        &[&target, &config_path],
        &format!("harness: update plugin {name} from {}", entry.id()),
    )?;
    println!(
        "Updated plugin {name}{}:",
        commit
            .as_deref()
            .map(|c| format!(" to {}", short(c)))
            .unwrap_or_default()
    );
    print_changes(&found);
    warn_about(name, contents, plugin.allow_hooks, plugin.allow_mcp);
    Ok(())
}

pub fn plugin_remove(project: &Path, name: &str) -> Result<()> {
    let repo = open_repo(project)?;
    plugin_ops::remove(&repo, name)?;
    println!("Removed plugin {name} from the project and from every role.");
    Ok(())
}

/// Downloads (if needed) and stages the plugin into `staged`.
fn fetch_and_stage(
    home: &Path,
    registry: &Registry,
    entry: &Entry,
    staged: &Path,
) -> Result<(Contents, Option<String>)> {
    let catalog_dir = registry.dir(home, &entry.catalog);
    let download = home
        .join(catalog::CATALOGS_DIR)
        .join(DOWNLOADS_DIR)
        .join(&entry.catalog)
        .join(&entry.name);
    let fetched = plugin_install::fetch(entry, &catalog_dir, &download)?;
    let _ = fs::remove_dir_all(staged);
    let contents = plugin_install::stage(entry, &fetched, staged)?;
    Ok((contents, fetched.commit))
}

/// The new version is prepared next to the plugin folder, then swapped in.
fn staging(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.new"))
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
    fn catalog_sources_become_clone_addresses() {
        assert_eq!(
            clone_url("anthropics/claude-plugins-official").unwrap(),
            "https://github.com/anthropics/claude-plugins-official.git"
        );
        for url in [
            "https://example.com/c.git",
            "git@github.com:o/r.git",
            "file:///tmp/c",
        ] {
            assert_eq!(clone_url(url).unwrap(), url);
        }
        for bad in ["just-a-word", "a/b/c", "../x/y z"] {
            assert!(clone_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn long_descriptions_are_cut() {
        assert_eq!(first_line("short\nsecond", 10), "short");
        assert_eq!(first_line("abcdefghijkl", 8), "abcde...");
    }
}
