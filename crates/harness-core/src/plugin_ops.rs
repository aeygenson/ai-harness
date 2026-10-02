//! Changes to the project's plugins that both `harness plugin ...` and the
//! TUI make: each one changes harness.toml and the plugin folder together and
//! commits them.

use std::fs;
use std::path::PathBuf;

use crate::config::{Config, CONFIG_FILE};
use crate::config_edit::{self, EditError};
use crate::git::{GitError, Repo, HARNESS_DIR};
use crate::plugins::{self, PLUGINS_DIR};

#[derive(Debug, thiserror::Error)]
pub enum OpsError {
    #[error("harness.toml has no [plugins.{0}]")]
    NoPlugin(String),
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

/// Removes `[plugins.<name>]`, the name from every role, and the plugin's
/// folder if it is in `.harness/plugins/`; a folder elsewhere in the
/// project is Lisa's own and stays. Commits `harness: remove plugin <name>`.
pub fn remove(repo: &Repo, name: &str) -> Result<(), OpsError> {
    let config_path = repo.root().join(HARNESS_DIR).join(CONFIG_FILE);
    let io = |path: &PathBuf| {
        let path = path.clone();
        move |source| OpsError::Io { path, source }
    };
    let text = fs::read_to_string(&config_path).map_err(io(&config_path))?;
    let config = Config::parse(&text)?;
    let plugin = config
        .plugins
        .get(name)
        .ok_or_else(|| OpsError::NoPlugin(name.to_string()))?;
    let folder = repo.root().join(plugins::relative_path(name, plugin));
    let new_text = config_edit::remove_plugin(&text, name)?;
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
    use crate::config::DEFAULT_CONFIG;
    use crate::projects;

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
