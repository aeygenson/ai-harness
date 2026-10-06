//! Saving changed settings, as the TUI does: the new `harness.toml` is checked
//! the same way as before a run (skills, MCP servers, plugins), and only then
//! written and committed. A file that does not pass stays as it was.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{Config, CONFIG_FILE};
use crate::git::{GitError, Repo, HARNESS_DIR};
use crate::mcp::{McpError, McpServers};
use crate::plugins::{PluginError, Plugins};
use crate::secret::Secret;
use crate::skills::{SkillError, Skills};

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("harness.toml is not valid: {0}")]
    Toml(#[from] toml::de::Error),
    #[error(transparent)]
    Skill(#[from] SkillError),
    #[error(transparent)]
    Mcp(#[from] McpError),
    #[error(transparent)]
    Plugin(#[from] PluginError),
    #[error("cannot write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Checks `text` as the settings of the project in `project_dir`. Secrets of
/// MCP servers are not needed for this: a missing one is reported when a role
/// runs, and `harness secret set` can add it any time.
pub fn check(project_dir: &Path, text: &str) -> Result<Config, SettingsError> {
    let config = Config::parse(text)?;
    Skills::load(&project_dir.join(HARNESS_DIR), &config)?;
    McpServers::load(&config, |_| Some(Secret::new("unused")))?;
    Plugins::load(project_dir, &config)?;
    Ok(config)
}

/// Checks `text`, writes it to `.harness/harness.toml` and commits it as
/// `harness: settings`. Returns false if nothing changed.
pub fn save(repo: &Repo, text: &str) -> Result<bool, SettingsError> {
    check(repo.root(), text)?;
    let path = repo.root().join(HARNESS_DIR).join(CONFIG_FILE);
    if fs::read_to_string(&path).is_ok_and(|old| old == text) {
        return Ok(false);
    }
    fs::write(&path, text).map_err(|source| SettingsError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(repo.commit_paths(&[&path], "harness: settings")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::projects;
    use crate::config::{AgentKind, DEFAULT_CONFIG};
    use crate::task::handoff::Role;

    #[test]
    fn only_settings_that_pass_the_checks_are_saved() {
        let dir = tempfile::tempdir().unwrap();
        projects::init(dir.path()).unwrap();
        let repo = Repo::open(dir.path()).unwrap();
        let path = dir.path().join(HARNESS_DIR).join(CONFIG_FILE);

        // Unchanged: nothing to commit.
        assert!(!save(&repo, DEFAULT_CONFIG).unwrap());

        // A skill without a file is refused and the file stays.
        let broken = DEFAULT_CONFIG.replace(
            "[roles.tester]\n",
            "[roles.tester]\nskills = [\"nothing-here\"]\n",
        );
        assert!(matches!(save(&repo, &broken), Err(SettingsError::Skill(_))));
        assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
        // So is an MCP server that is not described.
        let broken =
            DEFAULT_CONFIG.replace("[roles.tester]\n", "[roles.tester]\nmcp = [\"github\"]\n");
        assert!(matches!(save(&repo, &broken), Err(SettingsError::Mcp(_))));

        let good = DEFAULT_CONFIG.replace(
            "[roles.tester]\nagent = \"claude\"",
            "[roles.tester]\nagent = \"codex\"",
        );
        assert!(save(&repo, &good).unwrap());
        let config = Config::load(&dir.path().join(HARNESS_DIR)).unwrap();
        assert_eq!(config.roles[&Role::Tester].agent, AgentKind::Codex);
        assert!(repo.changed_files().unwrap().is_empty(), "committed");
    }
}
