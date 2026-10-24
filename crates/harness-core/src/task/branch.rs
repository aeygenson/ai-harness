//! The branch a task works on when the project is on GitHub (`[github]` in
//! `harness.toml`).
//!
//! Every new task gets its own branch, `harness/<task>`, so its work can
//! become one pull request. The branch starts at the commit checked out when
//! the task is created, not at the base branch: then every earlier task stays
//! in the history, so the task list and the next task id still see them while
//! their pull requests wait to be merged.

use crate::config::Config;
use crate::git::{GitError, Repo};
use crate::task::store::{following_task_id, task_ids, StoreError};

/// Every task branch starts with this, so the harness's branches are easy to
/// tell apart from the user's own.
pub const BRANCH_PREFIX: &str = "harness/";

/// The branch of a task: `task-001` -> `harness/task-001`.
pub fn task_branch(task_id: &str) -> String {
    format!("{BRANCH_PREFIX}{task_id}")
}

/// Why a task could not be put on its branch.
#[derive(Debug, thiserror::Error)]
pub enum BranchError {
    /// The new task's branch is already there, for example from a task whose
    /// folder was deleted.
    #[error(
        "the branch {0} already exists and may hold a task's work; check it out to look, \
         or create the task under another id"
    )]
    Exists(String),
    /// A new task's branch needs a project without uncommitted changes:
    /// they would silently move to the task's branch.
    #[error(
        "the new task would work on its own branch, but these files are not committed: {}. \
         Commit or remove them, then create the task again",
        .0.join(", ")
    )]
    UncommittedBeforeNew(Vec<String>),
    /// A branch needs a commit to start from.
    #[error("the project has no commits yet; commit something first, then create the task")]
    NoCommits,
    /// The task's branch is not checked out, and moving to it needs a
    /// project without uncommitted changes.
    #[error(
        "{task} works on the branch {branch}, but these files are not committed: {}. \
         Commit or remove them, then try again",
        files.join(", ")
    )]
    Uncommitted {
        /// The task, for example `task-001`.
        task: String,
        /// Its branch, for example `harness/task-001`.
        branch: String,
        /// The uncommitted files, as paths inside the project.
        files: Vec<String>,
    },
    /// Reading the task folders failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A git command failed.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Puts a new task on its own branch when the project is on GitHub; does
/// nothing otherwise. The files do not change, only the checked-out branch,
/// which stays checked out afterwards.
pub fn start(repo: &Repo, config: &Config, task_id: &str) -> Result<(), BranchError> {
    let branch = task_branch(task_id);
    // Even without `[github]` here (an older branch), a task with this id
    // may live on its branch: `enter` would later pick that one.
    if repo.has_branch(&branch)? {
        return Err(BranchError::Exists(branch));
    }
    if config.github.is_none() {
        return Ok(());
    }
    // In a repository without commits git would rename the empty branch
    // instead of making a new one, and the base branch would never exist.
    if repo.head()?.is_none() {
        return Err(BranchError::NoCommits);
    }
    let files = repo.changed_files()?;
    if !files.is_empty() {
        return Err(BranchError::UncommittedBeforeNew(files));
    }
    repo.create_branch(&branch)?;
    Ok(())
}

/// Checks out the task's branch, if it has one, before anything of the task
/// is read or saved, so the work lands there even if the user moved to another
/// branch in between. Returns `true` when it switched.
///
/// It does not look at `[github]`: the settings are read after the switch,
/// from the task's branch, and a branch `harness/<task>` exists only for a
/// task that was created with them.
pub fn enter(repo: &Repo, task_id: &str) -> Result<bool, BranchError> {
    let branch = task_branch(task_id);
    if repo.current_branch()?.as_deref() == Some(branch.as_str()) || !repo.has_branch(&branch)? {
        return Ok(false);
    }
    let files = repo.changed_files()?;
    if !files.is_empty() {
        return Err(BranchError::Uncommitted {
            task: task_id.to_string(),
            branch,
            files,
        });
    }
    repo.switch_branch(&branch)?;
    Ok(true)
}

/// The id for the next new task. Task branches count too: a task on another
/// branch is not in this branch's task folders, and its id must stay unused.
pub fn next_task_id(repo: &Repo) -> Result<String, BranchError> {
    let folders = task_ids(&repo.runs_dir())?;
    let branches = repo.branches_starting_with(BRANCH_PREFIX)?;
    let from_branches = branches
        .iter()
        .filter_map(|name| name.strip_prefix(BRANCH_PREFIX));
    Ok(following_task_id(
        folders.iter().map(String::as_str).chain(from_branches),
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const ON_GITHUB: &str = "[github]\nrepo = \"lisa/app\"\n";

    fn project() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        fs::write(dir.path().join("README.md"), "hello\n").unwrap();
        repo.commit_all("first").unwrap();
        (dir, repo)
    }

    #[test]
    fn a_project_on_github_starts_each_task_on_its_own_branch() {
        let (_dir, repo) = project();
        let config = Config::parse(ON_GITHUB).unwrap();

        start(&repo, &config, "task-001").unwrap();

        assert_eq!(
            repo.current_branch().unwrap().as_deref(),
            Some("harness/task-001")
        );
        let error = start(&repo, &config, "task-001").unwrap_err();
        assert!(matches!(error, BranchError::Exists(name) if name == "harness/task-001"));
    }

    #[test]
    fn a_new_task_never_takes_uncommitted_changes_to_its_branch() {
        let (dir, repo) = project();
        let before = repo.current_branch().unwrap();
        let config = Config::parse(ON_GITHUB).unwrap();
        fs::write(dir.path().join("README.md"), "changed\n").unwrap();

        let error = start(&repo, &config, "task-001").unwrap_err();

        assert!(
            matches!(&error, BranchError::UncommittedBeforeNew(files) if files == &["README.md"]),
            "{error}"
        );
        assert_eq!(repo.current_branch().unwrap(), before);
        assert!(!repo.has_branch("harness/task-001").unwrap());
    }

    #[test]
    fn a_project_without_commits_gets_no_task_branch() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let config = Config::parse(ON_GITHUB).unwrap();

        let error = start(&repo, &config, "task-001").unwrap_err();

        assert!(matches!(error, BranchError::NoCommits), "{error}");
    }

    #[test]
    fn an_id_on_a_task_branch_is_taken_even_without_github_here() {
        let (_dir, repo) = project();
        let base = repo.current_branch().unwrap().unwrap();
        repo.create_branch("harness/task-001").unwrap();
        repo.switch_branch(&base).unwrap();

        let error = start(&repo, &Config::parse("").unwrap(), "task-001").unwrap_err();

        assert!(matches!(error, BranchError::Exists(_)), "{error}");
    }

    #[test]
    fn a_project_not_on_github_stays_on_its_branch() {
        let (_dir, repo) = project();
        let before = repo.current_branch().unwrap();
        let config = Config::parse("").unwrap();

        start(&repo, &config, "task-001").unwrap();

        assert_eq!(repo.current_branch().unwrap(), before);
        assert!(!repo.has_branch("harness/task-001").unwrap());
    }

    #[test]
    fn entering_a_task_checks_out_its_branch() {
        let (_dir, repo) = project();
        let base = repo.current_branch().unwrap().unwrap();
        let config = Config::parse(ON_GITHUB).unwrap();
        start(&repo, &config, "task-001").unwrap();
        repo.switch_branch(&base).unwrap();

        assert!(enter(&repo, "task-001").unwrap());

        assert!(!enter(&repo, "task-001").unwrap());
        assert_eq!(
            repo.current_branch().unwrap().as_deref(),
            Some("harness/task-001")
        );
    }

    #[test]
    fn entering_a_task_never_takes_uncommitted_changes_along() {
        let (dir, repo) = project();
        let base = repo.current_branch().unwrap().unwrap();
        let config = Config::parse(ON_GITHUB).unwrap();
        start(&repo, &config, "task-001").unwrap();
        repo.switch_branch(&base).unwrap();
        fs::write(dir.path().join("README.md"), "changed\n").unwrap();

        let error = enter(&repo, "task-001").unwrap_err();

        assert!(
            matches!(&error, BranchError::Uncommitted { files, .. } if files == &["README.md"]),
            "{error}"
        );
        assert_eq!(repo.current_branch().unwrap(), Some(base));
    }

    #[test]
    fn a_task_without_a_branch_stays_where_it_is() {
        let (_dir, repo) = project();
        let before = repo.current_branch().unwrap();

        assert!(!enter(&repo, "task-001").unwrap());

        assert_eq!(repo.current_branch().unwrap(), before);
    }

    #[test]
    fn the_next_id_skips_tasks_on_other_branches() {
        let (_dir, repo) = project();
        let base = repo.current_branch().unwrap().unwrap();
        assert_eq!(next_task_id(&repo).unwrap(), "task-001");
        repo.create_branch("harness/task-004").unwrap();
        repo.switch_branch(&base).unwrap();

        assert_eq!(next_task_id(&repo).unwrap(), "task-005");
    }
}
