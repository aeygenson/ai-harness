//! Git's extra working folders ("worktrees"): one repository, several folders,
//! each with its own branch checked out. The harness gives each task on its
//! own branch a folder of its own, so the user's folder never changes branch.

use std::path::{Path, PathBuf};

use super::{GitError, Repo};

/// One working folder of the repository, as `git worktree list` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// The folder.
    pub path: PathBuf,
    /// The branch checked out there; `None` for a detached HEAD.
    pub branch: Option<String>,
    /// Git still knows the folder, but it is gone (deleted by hand); `git
    /// worktree prune` forgets it.
    pub missing: bool,
}

impl Repo {
    /// Every working folder of the repository, the main one first.
    pub fn worktrees(&self) -> Result<Vec<Worktree>, GitError> {
        let out = self.git(&["worktree", "list", "--porcelain"])?;
        Ok(parse_worktrees(&out))
    }

    /// The existing folder where the branch `name` is checked out, if any.
    /// A folder deleted by hand does not count.
    pub fn worktree_of(&self, name: &str) -> Result<Option<PathBuf>, GitError> {
        Ok(self
            .worktrees()?
            .into_iter()
            .find(|tree| !tree.missing && tree.branch.as_deref() == Some(name))
            .map(|tree| tree.path))
    }

    /// Makes git forget the working folders that were deleted by hand, so
    /// their branches can be checked out in a new folder.
    pub fn prune_worktrees(&self) -> Result<(), GitError> {
        self.git(&["worktree", "prune"])?;
        Ok(())
    }

    /// Makes the folder `path` with the existing branch `name` checked out.
    pub fn add_worktree(&self, path: &Path, name: &str) -> Result<(), GitError> {
        let path = path_text(path)?;
        self.git(&["worktree", "add", "-q", path, name])?;
        Ok(())
    }

    /// Makes a new branch `name` at the commit checked out here, and the
    /// folder `path` with it checked out. This folder stays as it is.
    pub fn add_worktree_with_branch(&self, path: &Path, name: &str) -> Result<(), GitError> {
        let path = path_text(path)?;
        self.git(&["worktree", "add", "-q", "-b", name, path, "HEAD"])?;
        Ok(())
    }

    /// Removes the working folder `path` (not its branch). Git refuses while
    /// it has uncommitted changes, so no work is lost.
    pub fn remove_worktree(&self, path: &Path) -> Result<(), GitError> {
        let path = path_text(path)?;
        self.git(&["worktree", "remove", path])?;
        Ok(())
    }

    /// Takes back a folder and branch made a moment ago by
    /// [`Repo::add_worktree_with_branch`] when what followed failed. The
    /// folder is removed even with files in it (they are only what was just
    /// made), and the branch is deleted even though it is not merged (it has
    /// no work of its own yet). Never call it for a folder with real work.
    pub fn discard_new_worktree(&self, path: &Path, name: &str) -> Result<(), GitError> {
        if path.exists() {
            let text = path_text(path)?;
            self.git(&["worktree", "remove", "--force", text])?;
        }
        self.prune_worktrees()?;
        if self.has_branch(name)? {
            self.git(&["branch", "-q", "-D", name])?;
        }
        Ok(())
    }
}

/// `path` as text for git; a path that is not valid Unicode is refused
/// rather than changed into another path.
fn path_text(path: &Path) -> Result<&str, GitError> {
    path.to_str().ok_or_else(|| GitError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the folder name is not valid Unicode",
        ),
    })
}

/// Reads `git worktree list --porcelain`: blocks of lines separated by an
/// empty line, `worktree <path>` first, `branch refs/heads/<name>` when a
/// branch is checked out and `prunable ...` when the folder is gone.
/// (`-z` would be safer for unusual names, but needs git 2.36.)
fn parse_worktrees(out: &str) -> Vec<Worktree> {
    // Some Windows builds of git end lines with `\r\n`.
    let out = out.replace("\r\n", "\n");
    let mut trees = Vec::new();
    for block in out.split("\n\n") {
        let mut path = None;
        let mut branch = None;
        let mut missing = false;
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(rest));
            } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
                branch = Some(rest.to_string());
            } else if line.starts_with("prunable") {
                missing = true;
            }
        }
        if let Some(path) = path {
            trees.push(Worktree {
                path,
                branch,
                missing,
            });
        }
    }
    trees
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn project() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        fs::create_dir(&root).unwrap();
        let repo = Repo::init(&root).unwrap();
        fs::write(root.join("README.md"), "hello\n").unwrap();
        repo.commit_all("first").unwrap();
        (dir, repo)
    }

    #[test]
    fn the_porcelain_list_gives_folders_and_branches() {
        let out = "worktree /a/app\r\nHEAD 1111\r\nbranch refs/heads/main\r\n\r\n\
                   worktree /a/app.tasks/task-001\nHEAD 2222\nbranch refs/heads/harness/task-001\n\n\
                   worktree /a/other\nHEAD 3333\ndetached\nprunable gitdir file points to \
                   non-existent location\n";

        let trees = parse_worktrees(out);

        assert_eq!(
            trees,
            [
                Worktree {
                    path: PathBuf::from("/a/app"),
                    branch: Some("main".into()),
                    missing: false,
                },
                Worktree {
                    path: PathBuf::from("/a/app.tasks/task-001"),
                    branch: Some("harness/task-001".into()),
                    missing: false,
                },
                Worktree {
                    path: PathBuf::from("/a/other"),
                    branch: None,
                    missing: true,
                },
            ]
        );
    }

    #[test]
    fn a_new_folder_gets_a_new_branch_and_the_first_folder_stays() {
        let (dir, repo) = project();
        let before = repo.current_branch().unwrap();
        let folder = dir.path().join("app.tasks/task-001");

        repo.add_worktree_with_branch(&folder, "harness/task-001")
            .unwrap();

        assert_eq!(repo.current_branch().unwrap(), before);
        assert!(folder.join("README.md").is_file());
        let found = repo.worktree_of("harness/task-001").unwrap().unwrap();
        assert_eq!(
            found.canonicalize().unwrap(),
            folder.canonicalize().unwrap()
        );
        let task = Repo::open(&folder).unwrap();
        assert_eq!(
            task.current_branch().unwrap().as_deref(),
            Some("harness/task-001")
        );
    }

    #[test]
    fn a_removed_folder_can_be_made_again_from_its_branch() {
        let (dir, repo) = project();
        let folder = dir.path().join("app.tasks/task-001");
        repo.add_worktree_with_branch(&folder, "harness/task-001")
            .unwrap();

        repo.remove_worktree(&folder).unwrap();
        assert!(!folder.exists());
        assert_eq!(repo.worktree_of("harness/task-001").unwrap(), None);
        repo.add_worktree(&folder, "harness/task-001").unwrap();

        assert!(folder.join("README.md").is_file());
    }

    #[test]
    fn a_folder_deleted_by_hand_is_forgotten_and_can_be_made_again() {
        let (dir, repo) = project();
        let folder = dir.path().join("app.tasks/task-001");
        repo.add_worktree_with_branch(&folder, "harness/task-001")
            .unwrap();
        fs::remove_dir_all(&folder).unwrap();

        assert_eq!(repo.worktree_of("harness/task-001").unwrap(), None);
        repo.prune_worktrees().unwrap();
        repo.add_worktree(&folder, "harness/task-001").unwrap();

        assert!(folder.join("README.md").is_file());
    }

    #[test]
    fn a_new_folder_and_its_branch_can_be_taken_back() {
        let (dir, repo) = project();
        let folder = dir.path().join("app.tasks/task-001");
        repo.add_worktree_with_branch(&folder, "harness/task-001")
            .unwrap();
        fs::write(folder.join("half-made"), "x").unwrap();

        repo.discard_new_worktree(&folder, "harness/task-001")
            .unwrap();

        assert!(!folder.exists());
        assert!(!repo.has_branch("harness/task-001").unwrap());
        assert_eq!(repo.worktrees().unwrap().len(), 1);
    }

    #[test]
    fn a_folder_with_uncommitted_work_is_not_removed() {
        let (dir, repo) = project();
        let folder = dir.path().join("app.tasks/task-001");
        repo.add_worktree_with_branch(&folder, "harness/task-001")
            .unwrap();
        fs::write(folder.join("README.md"), "changed\n").unwrap();

        repo.remove_worktree(&folder).unwrap_err();

        assert_eq!(
            fs::read_to_string(folder.join("README.md")).unwrap(),
            "changed\n"
        );
    }
}
