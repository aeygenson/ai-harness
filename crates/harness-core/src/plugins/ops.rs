//! Changes to plugins that both `harness marketplace/plugin ...` and the TUI
//! make. A plugin change edits harness.toml and the plugin folder together
//! and commits them; a catalog change touches only `~/.harness`.
//!
//! Catalogs are Lisa's, for all projects: the list is in
//! `~/.harness/marketplaces.toml`, the copies in `~/.harness/marketplaces/`.
//! A plugin is always copied into the project (`.harness/plugins/<name>/`) and
//! committed, so the agents only ever see what is in the project's git.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::edit::{self, EditError, NewPlugin};
use crate::config::{Config, CONFIG_FILE};
use crate::git::{self, GitError, Repo, HARNESS_DIR};
use crate::mcp::is_simple_name;
use crate::plugins::catalog::{self, Catalog, CatalogConfig, CatalogError, Entry, Registry};
use crate::plugins::install::{self, Changes, InstallError};
use crate::plugins::{self, Contents, PluginError, Plugins, PLUGINS_DIR};
use crate::task::handoff::Role;

/// A catalog being added is downloaded here first, before its name is known.
const ADDING_DIR: &str = ".adding";
/// Plugins that live in other repositories are downloaded here.
const DOWNLOADS_DIR: &str = ".downloads";

#[derive(Debug, thiserror::Error)]
pub enum OpsError {
    #[error("harness.toml has no [plugins.{0}]")]
    NoPlugin(String),
    #[error(
        "plugin name {0:?} cannot be used in a project; the harness needs lowercase \
         letters, digits, '-' and '_'"
    )]
    BadName(String),
    #[error("{0} already exists")]
    Exists(String),
    #[error("{0:?} is not a folder, `owner/repo` on GitHub or a git address")]
    BadSource(String),
    #[error("a catalog named {0:?} is already added")]
    CatalogExists(String),
    #[error("no catalog named {0:?}")]
    NoCatalog(String),
    #[error("plugin {name} came from catalog {catalog:?}, which is not added any more")]
    CatalogGone { name: String, catalog: String },
    #[error(
        "plugin {0} was not added from a catalog (it has no `source`), so it cannot be updated"
    )]
    NoSource(String),
    #[error("catalog {catalog} no longer lists {name}")]
    NotListed { catalog: String, name: String },
    #[error("cannot download {what}: {source}")]
    Download {
        what: String,
        #[source]
        source: GitError,
    },
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error("{0}")]
    Plugin(#[from] PluginError),
    #[error(transparent)]
    Edit(#[from] EditError),
    #[error("harness.toml is not valid: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("cannot change {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Git(#[from] GitError),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> OpsError {
    let path = path.to_path_buf();
    move |source| OpsError::Io { path, source }
}

/// `owner/repo` means GitHub; anything that looks like a git address is used as it is.
pub fn clone_url(source: &str) -> Result<String, OpsError> {
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
    Err(OpsError::BadSource(source.to_string()))
}

/// One added catalog, as the list of catalogs shows it.
#[derive(Debug)]
pub struct CatalogInfo {
    pub name: String,
    pub config: CatalogConfig,
    /// The commit of its copy; none for a local folder.
    pub commit: Option<String>,
    /// Its plugins, or why it cannot be read.
    pub entries: Result<Vec<Entry>, String>,
}

/// The added catalogs.
pub fn catalogs(home: &Path) -> Result<Vec<CatalogInfo>, OpsError> {
    let registry = Registry::load(home)?;
    Ok(registry
        .marketplaces
        .iter()
        .map(|(name, config)| {
            let dir = registry.dir(home, name);
            CatalogInfo {
                name: name.clone(),
                config: config.clone(),
                commit: if config.local {
                    None
                } else {
                    git::head_commit(&dir)
                },
                entries: Catalog::read(&dir, Some(name))
                    .map(|c| c.entries)
                    .map_err(|e| e.to_string()),
            }
        })
        .collect())
}

/// Adds a catalog: a local folder (read where it is), `owner/repo` on
/// GitHub or a git address (cloned). `name` replaces the catalog's own name.
pub fn add_catalog(home: &Path, source: &str, name: Option<&str>) -> Result<Catalog, OpsError> {
    let source = source.trim();
    let mut registry = Registry::load(home)?;
    let catalogs = home.join(catalog::CATALOGS_DIR);
    let local = Path::new(source);
    let (config, catalog) = if local.is_dir() {
        let path = local.canonicalize().map_err(io(local))?;
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
        git::fetch(&url, None, &adding).map_err(|source| OpsError::Download {
            what: format!("the catalog {url}"),
            source,
        })?;
        let catalog = match Catalog::read(&adding, name) {
            Ok(catalog) => catalog,
            Err(e) => {
                let _ = fs::remove_dir_all(&adding);
                return Err(e.into());
            }
        };
        if registry.marketplaces.contains_key(&catalog.name) {
            let _ = fs::remove_dir_all(&adding);
        } else {
            let target = catalogs.join(&catalog.name);
            let _ = fs::remove_dir_all(&target);
            fs::rename(&adding, &target).map_err(io(&target))?;
        }
        let config = CatalogConfig {
            source: url,
            local: false,
        };
        (config, catalog)
    };
    if registry.marketplaces.contains_key(&catalog.name) {
        return Err(OpsError::CatalogExists(catalog.name));
    }
    registry.marketplaces.insert(catalog.name.clone(), config);
    registry.save(home)?;
    Ok(catalog)
}

/// What updating a catalog did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogUpdate {
    /// A local folder is always read as it is.
    Local,
    /// Already at this commit.
    Same(String),
    /// Now at this commit.
    Updated(String),
}

/// Downloads the newest version of a catalog. Plugins already in projects
/// do not change.
pub fn update_catalog(home: &Path, name: &str) -> Result<CatalogUpdate, OpsError> {
    let registry = Registry::load(home)?;
    let config = registry
        .marketplaces
        .get(name)
        .ok_or_else(|| OpsError::NoCatalog(name.to_string()))?;
    if config.local {
        return Ok(CatalogUpdate::Local);
    }
    let dir = registry.dir(home, name);
    let before = git::head_commit(&dir);
    let after = git::fetch(&config.source, None, &dir).map_err(|source| OpsError::Download {
        what: format!("catalog {name}"),
        source,
    })?;
    Ok(if before.as_deref() == Some(after.as_str()) {
        CatalogUpdate::Same(after)
    } else {
        CatalogUpdate::Updated(after)
    })
}

/// Takes a catalog off the list and deletes its copy and downloads.
/// Plugins already in projects stay there.
pub fn remove_catalog(home: &Path, name: &str) -> Result<(), OpsError> {
    let mut registry = Registry::load(home)?;
    let config = registry
        .marketplaces
        .remove(name)
        .ok_or_else(|| OpsError::NoCatalog(name.to_string()))?;
    let catalogs = home.join(catalog::CATALOGS_DIR);
    if !config.local {
        let _ = fs::remove_dir_all(catalogs.join(name));
    }
    let _ = fs::remove_dir_all(catalogs.join(DOWNLOADS_DIR).join(name));
    registry.save(home)?;
    Ok(())
}

/// A plugin that was added to the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    /// The commit it was copied at, for a plugin from git.
    pub commit: Option<String>,
    /// What it brings that runs by itself.
    pub contents: Contents,
}

/// Copies the catalog's plugin `entry` into `.harness/plugins/<entry name>/`,
/// describes it in harness.toml (and gives it to `role`), checks it as a run
/// would and commits `harness: add plugin ...`. If the check fails, nothing
/// stays changed.
pub fn add(
    repo: &Repo,
    home: &Path,
    entry: &Entry,
    role: Option<Role>,
    allow_hooks: bool,
    allow_mcp: bool,
) -> Result<Added, OpsError> {
    let name = entry.name.as_str();
    if !is_simple_name(name) {
        return Err(OpsError::BadName(name.to_string()));
    }
    let config_path = repo.root().join(HARNESS_DIR).join(CONFIG_FILE);
    let text = fs::read_to_string(&config_path).map_err(io(&config_path))?;
    let target = repo.root().join(PLUGINS_DIR).join(name);
    if target.exists() {
        return Err(OpsError::Exists(target.display().to_string()));
    }
    let registry = Registry::load(home)?;
    let staged = staging(&target);
    let (contents, commit) = fetch_and_stage(home, &registry, entry, &staged)?;
    let new_text = edit::add_plugin(
        &text,
        &NewPlugin {
            name,
            agent: entry.agent,
            source: &entry.id(),
            commit: commit.as_deref(),
            allow_hooks,
            allow_mcp,
        },
        role,
    );
    let new_text = match new_text {
        Ok(text) => text,
        Err(e) => {
            let _ = fs::remove_dir_all(&staged);
            return Err(e.into());
        }
    };
    install::put_in_place(&staged, &target).map_err(io(&target))?;
    fs::write(&config_path, &new_text).map_err(io(&config_path))?;
    let checked = Config::parse(&new_text)
        .map_err(OpsError::from)
        .and_then(|config| Ok(Plugins::load(repo.root(), &config)?));
    if let Err(e) = checked {
        let _ = fs::remove_dir_all(&target);
        let _ = fs::write(&config_path, &text);
        return Err(e);
    }
    repo.commit_paths(
        &[&target, &config_path],
        &format!("harness: add plugin {name} from {}", entry.id()),
    )?;
    Ok(Added { commit, contents })
}

/// A new version of a plugin, downloaded and ready next to the old one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub name: String,
    /// `<catalog>/<plugin>`.
    pub id: String,
    target: PathBuf,
    staged: PathBuf,
    pub commit: Option<String>,
    pub changes: Changes,
    pub contents: Contents,
}

/// Downloads the version of plugin `name` that its catalog lists now (the
/// catalog itself is updated with `update_catalog`). `None`: nothing changed.
/// The new version waits beside the plugin until `apply_update` or `discard`.
pub fn prepare_update(repo: &Repo, home: &Path, name: &str) -> Result<Option<Prepared>, OpsError> {
    let config_path = repo.root().join(HARNESS_DIR).join(CONFIG_FILE);
    let text = fs::read_to_string(&config_path).map_err(io(&config_path))?;
    let config = Config::parse(&text)?;
    let plugin = config
        .plugins
        .get(name)
        .ok_or_else(|| OpsError::NoPlugin(name.to_string()))?;
    let (catalog_name, entry_name) = plugin
        .source
        .as_deref()
        .and_then(|s| s.split_once('/'))
        .ok_or_else(|| OpsError::NoSource(name.to_string()))?;
    let registry = Registry::load(home)?;
    if !registry.marketplaces.contains_key(catalog_name) {
        return Err(OpsError::CatalogGone {
            name: name.to_string(),
            catalog: catalog_name.to_string(),
        });
    }
    let catalog = Catalog::read(&registry.dir(home, catalog_name), Some(catalog_name))?;
    let entry = catalog
        .entries
        .iter()
        .find(|e| e.name == entry_name && e.agent == plugin.agent)
        .ok_or_else(|| OpsError::NotListed {
            catalog: catalog_name.to_string(),
            name: entry_name.to_string(),
        })?;
    let target = repo.root().join(plugins::relative_path(name, plugin));
    let staged = staging(&target);
    let (contents, commit) = fetch_and_stage(home, &registry, entry, &staged)?;
    let changes = install::changes(&target, &staged).map_err(io(&target))?;
    if changes.is_empty() {
        let _ = fs::remove_dir_all(&staged);
        return Ok(None);
    }
    Ok(Some(Prepared {
        name: name.to_string(),
        id: entry.id(),
        target,
        staged,
        commit,
        changes,
        contents,
    }))
}

/// Takes the new version: it replaces the plugin folder, `commit` changes,
/// and the project is checked as before a run. If the check fails, the old
/// version stays. Commits `harness: update plugin ...`.
pub fn apply_update(repo: &Repo, prepared: &Prepared) -> Result<(), OpsError> {
    let name = &prepared.name;
    let config_path = repo.root().join(HARNESS_DIR).join(CONFIG_FILE);
    let text = fs::read_to_string(&config_path).map_err(io(&config_path))?;
    let target = &prepared.target;
    let old = target.with_file_name(format!(".{name}.old"));
    let _ = fs::remove_dir_all(&old);
    fs::rename(target, &old).map_err(io(target))?;
    install::put_in_place(&prepared.staged, target).map_err(io(target))?;
    let new_text = match &prepared.commit {
        Some(commit) => edit::set_plugin_commit(&text, name, commit)?,
        None => text.clone(),
    };
    fs::write(&config_path, &new_text).map_err(io(&config_path))?;
    let checked = Config::parse(&new_text)
        .map_err(OpsError::from)
        .and_then(|config| Ok(Plugins::load(repo.root(), &config)?));
    if let Err(e) = checked {
        let _ = fs::remove_dir_all(target);
        let _ = fs::rename(&old, target);
        let _ = fs::write(&config_path, &text);
        return Err(e);
    }
    let _ = fs::remove_dir_all(&old);
    repo.commit_paths(
        &[target, &config_path],
        &format!("harness: update plugin {name} from {}", prepared.id),
    )?;
    Ok(())
}

/// Drops a new version that was not taken.
pub fn discard(prepared: &Prepared) {
    let _ = fs::remove_dir_all(&prepared.staged);
}

/// Downloads (if needed) and stages the plugin into `staged`.
fn fetch_and_stage(
    home: &Path,
    registry: &Registry,
    entry: &Entry,
    staged: &Path,
) -> Result<(Contents, Option<String>), OpsError> {
    let catalog_dir = registry.dir(home, &entry.catalog);
    let download = home
        .join(catalog::CATALOGS_DIR)
        .join(DOWNLOADS_DIR)
        .join(&entry.catalog)
        .join(&entry.name);
    let fetched = install::fetch(entry, &catalog_dir, &download)?;
    let _ = fs::remove_dir_all(staged);
    let contents = install::stage(entry, &fetched, staged)?;
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

/// Removes `[plugins.<name>]`, the name from every role, and the plugin's
/// folder if it is in `.harness/plugins/`; a folder elsewhere in the
/// project is Lisa's own and stays. Commits `harness: remove plugin <name>`.
pub fn remove(repo: &Repo, name: &str) -> Result<(), OpsError> {
    let config_path = repo.root().join(HARNESS_DIR).join(CONFIG_FILE);
    let text = fs::read_to_string(&config_path).map_err(io(&config_path))?;
    let config = Config::parse(&text)?;
    let plugin = config
        .plugins
        .get(name)
        .ok_or_else(|| OpsError::NoPlugin(name.to_string()))?;
    let folder = repo.root().join(plugins::relative_path(name, plugin));
    let new_text = edit::remove_plugin(&text, name)?;
    fs::write(&config_path, new_text).map_err(io(&config_path))?;
    if folder.starts_with(repo.root().join(PLUGINS_DIR)) && folder.exists() {
        fs::remove_dir_all(&folder).map_err(io(&folder))?;
    }
    repo.commit_paths(
        &[&folder, &config_path],
        &format!("harness: remove plugin {name}"),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::projects;
    use crate::config::DEFAULT_CONFIG;

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

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// A local catalog folder with Claude plugins `review` (with hooks) and `notes`.
    fn local_catalog(dir: &Path) {
        write(
            &dir.join(".claude-plugin/marketplace.json"),
            r#"{"name": "mine", "plugins": [
                {"name": "review", "description": "Reviews code", "source": "./plugins/review"},
                {"name": "notes", "source": "./plugins/notes"}
            ]}"#,
        );
        write(
            &dir.join("plugins/review/.claude-plugin/plugin.json"),
            r#"{"name": "review"}"#,
        );
        write(&dir.join("plugins/review/hooks/hooks.json"), "{}");
        write(
            &dir.join("plugins/notes/.claude-plugin/plugin.json"),
            r#"{"name": "notes"}"#,
        );
        write(&dir.join("plugins/notes/commands/note.md"), "# note");
    }

    #[test]
    fn plugins_come_from_a_catalog_and_are_updated_from_it() {
        let home = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        local_catalog(source.path());
        let project = tempfile::tempdir().unwrap();
        projects::init(project.path()).unwrap();
        let repo = Repo::open(project.path()).unwrap();

        // A local folder is added as it is, and listed.
        let catalog = add_catalog(home.path(), &source.path().display().to_string(), None).unwrap();
        assert_eq!((catalog.name.as_str(), catalog.entries.len()), ("mine", 2));
        assert!(matches!(
            add_catalog(home.path(), &source.path().display().to_string(), None),
            Err(OpsError::CatalogExists(_))
        ));
        let listed = catalogs(home.path()).unwrap();
        assert_eq!(listed[0].entries.as_ref().unwrap().len(), 2);
        assert_eq!(
            update_catalog(home.path(), "mine").unwrap(),
            CatalogUpdate::Local
        );
        let entry = |name: &str| {
            listed[0]
                .entries
                .as_ref()
                .unwrap()
                .iter()
                .find(|e| e.name == name)
                .unwrap()
                .clone()
        };

        // A plugin with hooks cannot go to a role before they are allowed:
        // nothing stays.
        let err = add(
            &repo,
            home.path(),
            &entry("review"),
            Some(Role::Architect),
            false,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("hooks"), "{err}");
        assert!(!project.path().join(PLUGINS_DIR).join("review").exists());
        assert!(repo.changed_files().unwrap().is_empty());
        // Without a role it is added, and says what it brings.
        let added = add(&repo, home.path(), &entry("review"), None, false, false).unwrap();
        assert!(added.contents.hooks);
        let added = add(
            &repo,
            home.path(),
            &entry("notes"),
            Some(Role::Tester),
            false,
            false,
        )
        .unwrap();
        assert!(!added.contents.hooks);
        let config = Config::load(&project.path().join(HARNESS_DIR)).unwrap();
        assert_eq!(
            config.plugins["notes"].source.as_deref(),
            Some("mine/notes")
        );
        assert_eq!(config.roles[&Role::Tester].plugins, ["notes"]);
        assert!(repo.changed_files().unwrap().is_empty());
        assert!(matches!(
            add(&repo, home.path(), &entry("notes"), None, false, false),
            Err(OpsError::Exists(_))
        ));

        // Nothing new: no update.
        assert!(prepare_update(&repo, home.path(), "notes")
            .unwrap()
            .is_none());
        // A new version waits until it is taken or dropped.
        write(
            &source.path().join("plugins/notes/commands/more.md"),
            "# more",
        );
        let prepared = prepare_update(&repo, home.path(), "notes")
            .unwrap()
            .unwrap();
        assert_eq!(prepared.changes.added, ["commands/more.md"]);
        discard(&prepared);
        assert!(repo.changed_files().unwrap().is_empty());
        let prepared = prepare_update(&repo, home.path(), "notes")
            .unwrap()
            .unwrap();
        apply_update(&repo, &prepared).unwrap();
        let folder = project.path().join(PLUGINS_DIR).join("notes");
        assert!(folder.join("commands/more.md").is_file());
        assert!(repo.changed_files().unwrap().is_empty());

        // Removing the catalog keeps the plugins in the project.
        remove_catalog(home.path(), "mine").unwrap();
        assert!(catalogs(home.path()).unwrap().is_empty());
        assert!(folder.is_dir());
        assert!(matches!(
            prepare_update(&repo, home.path(), "notes"),
            Err(OpsError::CatalogGone { .. })
        ));
    }

    #[test]
    fn a_removed_plugin_leaves_the_roles_the_settings_and_git() {
        let dir = tempfile::tempdir().unwrap();
        projects::init(dir.path()).unwrap();
        let repo = Repo::open(dir.path()).unwrap();
        let folder = dir.path().join(PLUGINS_DIR).join("review");
        fs::create_dir_all(folder.join(".claude-plugin")).unwrap();
        fs::write(folder.join(".claude-plugin/plugin.json"), "{}").unwrap();
        let config_path = dir.path().join(HARNESS_DIR).join(CONFIG_FILE);
        let text = format!("{DEFAULT_CONFIG}\n[plugins.review]\nagent = \"claude\"\n").replace(
            "[roles.architect]\n",
            "[roles.architect]\nplugins = [\"review\"]\n",
        );
        fs::write(&config_path, &text).unwrap();
        repo.commit_all("add review").unwrap();

        remove(&repo, "review").unwrap();
        let config = Config::load(&dir.path().join(HARNESS_DIR)).unwrap();
        assert!(!config.plugins.contains_key("review"));
        assert!(config.roles.values().all(|r| r.plugins.is_empty()));
        assert!(!folder.exists());
        assert!(repo.changed_files().unwrap().is_empty());
        assert!(matches!(
            remove(&repo, "review"),
            Err(OpsError::NoPlugin(_))
        ));
    }
}
