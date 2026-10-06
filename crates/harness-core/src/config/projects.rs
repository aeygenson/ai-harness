//! The projects Lisa works on, so the TUI can list them and switch between them.
//!
//! The list lives outside every project, in `~/.harness/projects.toml`:
//!
//! ```toml
//! last = "/home/alex/code/harness-test"
//!
//! [[projects]]
//! name = "harness-test"
//! path = "/home/alex/code/harness-test"
//! ```
//!
//! Removing a project from the list never deletes its folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{CONFIG_FILE, DEFAULT_CONFIG};
use crate::git::{GitError, Repo, HARNESS_DIR};

/// The name of the project list file inside `~/.harness`.
pub const PROJECTS_FILE: &str = "projects.toml";

/// Why the project list or a new project could not be read, saved or set up.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    /// A file or folder could not be read, written or created.
    #[error("cannot access {path}: {source}")]
    Io {
        /// The file or folder involved.
        path: PathBuf,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },
    /// `projects.toml` was read but is not a valid project list.
    #[error("{path} is not valid: {source}")]
    Toml {
        /// The path of the file that is not valid.
        path: PathBuf,
        /// What is wrong, with the line, from the TOML reader.
        #[source]
        source: toml::de::Error,
    },
    /// The project list could not be turned into TOML.
    #[error("cannot save the project list: {0}")]
    Save(#[from] toml::ser::Error),
    /// A git command for the new project failed.
    #[error(transparent)]
    Git(#[from] GitError),
    /// The path exists but is a file, so it cannot hold a project.
    #[error("{0} exists and is not a folder")]
    NotAFolder(PathBuf),
}

/// The list of Lisa's projects, as stored in `~/.harness/projects.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Projects {
    /// The project opened last; the TUI starts with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<PathBuf>,
    /// Every project on the list, in the order they were added.
    #[serde(default)]
    pub projects: Vec<Project>,
}

/// One project on the list: a `[[projects]]` entry in `projects.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// The name shown in the TUI.
    pub name: String,
    /// The project's folder.
    pub path: PathBuf,
}

/// `~/.harness`, where the project list and plugin catalogs live.
pub fn harness_home() -> Option<PathBuf> {
    harness_platform::home::harness_dir()
}

impl Projects {
    /// Reads `<home>/projects.toml`; a missing file is an empty list.
    pub fn load(home: &Path) -> Result<Self, ProjectError> {
        let path = home.join(PROJECTS_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(ProjectError::Io { path, source }),
        };
        toml::from_str(&text).map_err(|source| ProjectError::Toml { path, source })
    }

    /// Writes the list to `<home>/projects.toml`, creating `home` if needed.
    pub fn save(&self, home: &Path) -> Result<(), ProjectError> {
        let path = home.join(PROJECTS_FILE);
        let text = toml::to_string(self)?;
        fs::create_dir_all(home)
            .and_then(|()| fs::write(&path, text))
            .map_err(|source| ProjectError::Io { path, source })
    }

    /// Adds the project, or renames it if the folder is already listed.
    pub fn add(&mut self, name: &str, path: &Path) {
        match self.projects.iter_mut().find(|p| p.path == path) {
            Some(project) => project.name = name.to_string(),
            None => self.projects.push(Project {
                name: name.to_string(),
                path: path.to_path_buf(),
            }),
        }
    }

    /// Takes the project off the list. Its folder stays.
    pub fn remove(&mut self, path: &Path) {
        self.projects.retain(|p| p.path != path);
        if self.last.as_deref() == Some(path) {
            self.last = None;
        }
    }
}

/// What `init` had to do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Initialized {
    /// True if the project folder did not exist and was created.
    pub created_folder: bool,
    /// True if a new git repository was created in the folder.
    pub created_git: bool,
    /// True if `.harness/harness.toml` was written with the default settings.
    pub created_config: bool,
}

/// Prepares `root` for the harness: creates the folder, a git repository if
/// the folder is not the top of one, and `.harness/harness.toml`, then commits
/// the settings. Existing files are left as they are.
pub fn init(root: &Path) -> Result<Initialized, ProjectError> {
    let io_error = |source| ProjectError::Io {
        path: root.to_path_buf(),
        source,
    };
    let mut done = Initialized::default();
    if root.exists() && !root.is_dir() {
        return Err(ProjectError::NotAFolder(root.to_path_buf()));
    }
    if !root.exists() {
        fs::create_dir_all(root).map_err(io_error)?;
        done.created_folder = true;
    }
    // A folder inside another repository gets its own, so the harness never
    // commits into the outer one.
    let repo = match Repo::open(root) {
        Ok(repo) if repo.is_top_level()? => repo,
        _ => {
            done.created_git = true;
            Repo::init(root)?
        }
    };
    let harness_dir = root.join(HARNESS_DIR);
    let config = harness_dir.join(CONFIG_FILE);
    if !config.exists() {
        fs::create_dir_all(&harness_dir)
            .and_then(|()| fs::write(&config, DEFAULT_CONFIG))
            .map_err(|source| ProjectError::Io {
                path: config.clone(),
                source,
            })?;
        done.created_config = true;
    }
    repo.ensure_harness_ignores()?;
    repo.commit_paths(&[&config], "harness: settings")?;
    Ok(done)
}

/// A project name from its folder: `~/code/harness-test` -> `harness-test`.
pub fn name_of(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn the_list_is_saved_and_read_back() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(Projects::load(home.path()).unwrap(), Projects::default());

        let mut projects = Projects::default();
        projects.add("test", Path::new("/code/test"));
        projects.add("other", Path::new("/code/other"));
        projects.add("renamed", Path::new("/code/test"));
        projects.last = Some("/code/other".into());
        projects.save(home.path()).unwrap();

        let read = Projects::load(home.path()).unwrap();
        assert_eq!(read, projects);
        assert_eq!(read.projects[0].name, "renamed");
        assert_eq!(read.projects.len(), 2);

        let mut read = read;
        read.remove(Path::new("/code/other"));
        assert_eq!(read.projects.len(), 1);
        assert_eq!(read.last, None);

        fs::write(home.path().join(PROJECTS_FILE), "nonsense = 1").unwrap();
        assert!(matches!(
            Projects::load(home.path()),
            Err(ProjectError::Toml { .. })
        ));
    }

    #[test]
    fn init_creates_the_folder_git_and_settings_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("new-project");
        let done = init(&root).unwrap();
        assert_eq!(
            done,
            Initialized {
                created_folder: true,
                created_git: true,
                created_config: true
            }
        );
        let config = Config::load(&root.join(HARNESS_DIR)).unwrap();
        assert!(config.retro.is_some());
        let repo = Repo::open(&root).unwrap();
        assert!(repo.changed_files().unwrap().is_empty());

        // A second time nothing is created.
        assert_eq!(init(&root).unwrap(), Initialized::default());
        assert_eq!(name_of(&root), "new-project");
    }

    #[test]
    fn a_folder_inside_another_repository_gets_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let outer = Repo::init(dir.path()).unwrap();
        let inner = dir.path().join("inner");
        fs::create_dir(&inner).unwrap();
        assert!(init(&inner).unwrap().created_git);
        assert!(inner.join(".git").exists());
        assert_eq!(outer.head().unwrap(), None, "nothing committed outside");

        let file = dir.path().join("file");
        fs::write(&file, "").unwrap();
        assert!(matches!(init(&file), Err(ProjectError::NotAFolder(_))));
    }
}
