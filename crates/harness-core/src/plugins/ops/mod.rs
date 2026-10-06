//! Changes to plugins that both `harness marketplace/plugin ...` and the TUI
//! make. A plugin change edits harness.toml and the plugin folder together
//! and commits them; a catalog change touches only `~/.harness`.
//!
//! Catalogs are Lisa's, for all projects: the list is in
//! `~/.harness/marketplaces.toml`, the copies in `~/.harness/marketplaces/`.
//! A plugin is always copied into the project (`.harness/plugins/<name>/`) and
//! committed, so the agents only ever see what is in the project's git.

mod catalogs;

pub use catalogs::{
    add_catalog, catalogs, clone_url, remove_catalog, update_catalog, CatalogInfo, CatalogUpdate,
};

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::edit::{self, EditError, NewPlugin};
use crate::config::{Config, CONFIG_FILE};
use crate::git::{GitError, Repo, HARNESS_DIR};
use crate::mcp::is_simple_name;
use crate::plugins::catalog::{self, Catalog, CatalogError, Entry, Registry};
use crate::plugins::install::{self, Changes, InstallError};
use crate::plugins::{self, Contents, PluginError, Plugins, PLUGINS_DIR};
use crate::task::handoff::Role;

/// A catalog being added is downloaded here first, before its name is known.
const ADDING_DIR: &str = ".adding";
/// Plugins that live in other repositories are downloaded here.
const DOWNLOADS_DIR: &str = ".downloads";

/// Why adding, updating or removing a plugin or a catalog failed.
#[derive(Debug, thiserror::Error)]
pub enum OpsError {
    /// harness.toml has no plugin with this name.
    #[error("harness.toml has no [plugins.{0}]")]
    NoPlugin(String),
    /// The plugin's name is not a simple lowercase name the project can use.
    #[error(
        "plugin name {0:?} cannot be used in a project; the harness needs lowercase \
         letters, digits, '-' and '_'"
    )]
    BadName(String),
    /// The plugin's folder is already in the project; holds its path.
    #[error("{0} already exists")]
    Exists(String),
    /// The catalog source is neither a folder, `owner/repo` nor a git address.
    #[error("{0:?} is not a folder, `owner/repo` on GitHub or a git address")]
    BadSource(String),
    /// A catalog with this name is already added.
    #[error("a catalog named {0:?} is already added")]
    CatalogExists(String),
    /// No catalog with this name is added.
    #[error("no catalog named {0:?}")]
    NoCatalog(String),
    /// The catalog the plugin came from was removed since.
    #[error("plugin {name} came from catalog {catalog:?}, which is not added any more")]
    CatalogGone {
        /// The plugin's name in harness.toml.
        name: String,
        /// The catalog named in the plugin's `source`.
        catalog: String,
    },
    /// The plugin has no `source` in harness.toml, so there is nothing to update from.
    #[error(
        "plugin {0} was not added from a catalog (it has no `source`), so it cannot be updated"
    )]
    NoSource(String),
    /// The catalog no longer offers the plugin.
    #[error("catalog {catalog} no longer lists {name}")]
    NotListed {
        /// The catalog named in the plugin's `source`.
        catalog: String,
        /// The plugin's name in that catalog.
        name: String,
    },
    /// Downloading a catalog with git failed.
    #[error("cannot download {what}: {source}")]
    Download {
        /// What was being downloaded, ready for the message (for example `catalog <name>`).
        what: String,
        /// The git error that stopped the download.
        #[source]
        source: GitError,
    },
    /// The catalog could not be read.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// The plugin could not be fetched or copied.
    #[error(transparent)]
    Install(#[from] InstallError),
    /// The plugin failed the checks a run would make.
    #[error("{0}")]
    Plugin(#[from] PluginError),
    /// harness.toml could not be changed.
    #[error(transparent)]
    Edit(#[from] EditError),
    /// harness.toml could not be parsed.
    #[error("harness.toml is not valid: {0}")]
    Toml(#[from] toml::de::Error),
    /// Reading or writing a file or folder failed.
    #[error("cannot change {path}: {source}")]
    Io {
        /// The file or folder that could not be changed.
        path: PathBuf,
        /// The underlying file system error.
        #[source]
        source: std::io::Error,
    },
    /// A git command (such as the commit) failed.
    #[error(transparent)]
    Git(#[from] GitError),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> OpsError {
    let path = path.to_path_buf();
    move |source| OpsError::Io { path, source }
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
    /// The plugin's name in harness.toml.
    pub name: String,
    /// `<catalog>/<plugin>`.
    pub id: String,
    target: PathBuf,
    staged: PathBuf,
    /// The git commit of the new version; `None` when it is not known.
    pub commit: Option<String>,
    /// Files that differ between the installed version and the new one.
    pub changes: Changes,
    /// What the new version brings that runs by itself.
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
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
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
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
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
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
        let prepared = prepare_update(&repo, home.path(), "notes")
            .unwrap()
            .unwrap();
        apply_update(&repo, &prepared).unwrap();
        let folder = project.path().join(PLUGINS_DIR).join("notes");
        assert!(folder.join("commands/more.md").is_file());
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());

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
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
        assert!(matches!(
            remove(&repo, "review"),
            Err(OpsError::NoPlugin(_))
        ));
    }
}
