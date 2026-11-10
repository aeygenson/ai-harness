//! Saving changed settings, as the TUI does: the new `harness.toml` is checked
//! the same way as before a run (skills, MCP servers, plugins), and only then
//! written and committed. A file that does not pass stays as it was.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::edit::{self, EditError};
use crate::config::{Config, CONFIG_FILE};
use crate::git::{GitError, Repo, HARNESS_DIR};
use crate::mcp::{McpError, McpServers};
use crate::plugins::{PluginError, Plugins};
use crate::secret::Secret;
use crate::skills::{SkillError, Skills};
use crate::task::handoff::Role;
use crate::task::run_lock::{self, RunLockError};

/// Why changed settings were not saved; the old file then stays as it was.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// The text is not valid `harness.toml` settings.
    #[error("harness.toml is not valid: {0}")]
    Toml(#[from] toml::de::Error),
    /// A skill named in the settings is missing or broken.
    #[error(transparent)]
    Skill(#[from] SkillError),
    /// An MCP server in the settings is not described correctly.
    #[error(transparent)]
    Mcp(#[from] McpError),
    /// A plugin in the settings is missing or not allowed.
    #[error(transparent)]
    Plugin(#[from] PluginError),
    /// The file could not be written.
    #[error("cannot write {path}: {source}")]
    Io {
        /// The file that could not be written.
        path: PathBuf,
        /// The error from the operating system.
        #[source]
        source: std::io::Error,
    },
    /// Committing the changed file failed.
    #[error(transparent)]
    Git(#[from] GitError),
    /// The change could not be made to the text of harness.toml.
    #[error(transparent)]
    Edit(#[from] EditError),
    /// A role is working in this project, so its permissions stay as they are.
    #[error("{0} is running; change the permissions when it stops")]
    TaskRunning(String),
    /// Whether a role is working could not be found out.
    #[error(transparent)]
    RunLock(#[from] RunLockError),
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
    // Whole: a window open on the same project never reads half of it.
    harness_platform::whole_file::write(&path, text.as_bytes()).map_err(|source| {
        SettingsError::Io {
            path: path.clone(),
            source,
        }
    })?;
    Ok(repo.commit_paths(&[&path], "harness: settings")?)
}

/// Allows `command` to `role` (adds it to `extra_commands`), or with
/// `allow` false takes it away, then checks, writes and commits harness.toml.
/// Returns false if nothing changed. Used by `harness allow-command` and
/// the TUI's Retro tab.
pub fn set_extra_command(
    repo: &Repo,
    role: Role,
    command: &str,
    allow: bool,
) -> Result<bool, SettingsError> {
    // A role that is working keeps the rights it started with; the commit
    // would also land in the middle of its work.
    if let Some(working) = run_lock::working(repo)? {
        return Err(SettingsError::TaskRunning(working.task));
    }
    let text = read_config(repo)?;
    let changed = if allow {
        edit::add_extra_command(&text, role, command)?
    } else {
        edit::remove_extra_command(&text, role, command)?
    };
    save(repo, &changed)
}

/// The text of the project's `harness.toml`.
fn read_config(repo: &Repo) -> Result<String, SettingsError> {
    let path = repo.root().join(HARNESS_DIR).join(CONFIG_FILE);
    fs::read_to_string(&path).map_err(|source| SettingsError::Io { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::projects;
    use crate::config::{AgentKind, DEFAULT_CONFIG};

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

    #[test]
    fn an_extra_command_is_allowed_committed_and_taken_back() {
        let dir = tempfile::tempdir().unwrap();
        projects::init(dir.path()).unwrap();
        let repo = Repo::open(dir.path()).unwrap();
        let harness = dir.path().join(HARNESS_DIR);

        assert!(set_extra_command(&repo, Role::Security, "trivy fs", true).unwrap());
        let config = Config::load(&harness).unwrap();
        assert_eq!(config.roles[&Role::Security].extra_commands, ["trivy fs"]);
        assert!(repo.changed_files().unwrap().is_empty(), "committed");

        // A command that would allow too much is refused; the file stays.
        set_extra_command(&repo, Role::Security, "bash", true).unwrap_err();
        assert!(set_extra_command(&repo, Role::Security, "trivy fs", false).unwrap());
        let config = Config::load(&harness).unwrap();
        assert_eq!(
            config.roles[&Role::Security].extra_commands,
            Vec::<String>::new()
        );
    }

    #[test]
    fn no_command_is_allowed_while_a_task_runs() {
        let dir = tempfile::tempdir().unwrap();
        projects::init(dir.path()).unwrap();
        let repo = Repo::open(dir.path()).unwrap();
        let mut lock = run_lock::RunLock::hold(&repo).unwrap();
        lock.set_task("task-007").unwrap();

        let error = set_extra_command(&repo, Role::Security, "trivy fs", true).unwrap_err();
        assert!(matches!(error, SettingsError::TaskRunning(ref task) if task == "task-007"));
        let config = Config::load(&dir.path().join(HARNESS_DIR)).unwrap();
        assert_eq!(
            config.roles[&Role::Security].extra_commands,
            Vec::<String>::new()
        );
    }
}
