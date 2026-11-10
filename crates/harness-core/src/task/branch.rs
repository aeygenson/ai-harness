//! The branch and folder a task works in when the project is on GitHub
//! (`[github]` in `harness.toml`).
//!
//! Every new task gets its own branch, `harness/<task>`, so its work can
//! become one pull request, and its own working folder next to the project,
//! `<project>.tasks/<task>` (a git worktree), so the user's folder never
//! changes branch under them. The branch starts at the commit checked out
//! when the task is created, not at the base branch: then every earlier task
//! stays in the history, so the task list and the next task id still see them
//! while their pull requests wait to be merged.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::git::{GitError, Repo, HARNESS_DIR};
use crate::task::store::{following_task_id, task_ids, StoreError};

/// Every task branch starts with this, so the harness's branches are easy to
/// tell apart from the user's own.
pub const BRANCH_PREFIX: &str = "harness/";

/// The end of the name of the folder next to the project that holds the
/// tasks' folders: `app` -> `app.tasks`.
const TASKS_FOLDER_SUFFIX: &str = ".tasks";

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
    /// A branch needs a commit to start from.
    #[error("the project has no commits yet; commit something first, then create the task")]
    NoCommits,
    /// The project's folder has no name to put the tasks' folders next to
    /// (for example the root of a drive).
    #[error("the project folder {0} has no name; the tasks' folders cannot be put next to it")]
    NoFolderName(PathBuf),
    /// Something is already where the new task's folder would go.
    #[error("{0} is already there; move it away or delete it, then create the task again")]
    FolderInTheWay(PathBuf),
    /// The task's branch is checked out in a folder the harness did not
    /// make, so the harness does not work there.
    #[error(
        "the branch {branch} is checked out in {folder}; switch that folder to another \
         branch (or remove it with `git worktree remove`), then try again"
    )]
    ElsewhereFolder {
        /// The task's branch.
        branch: String,
        /// The folder where it is checked out.
        folder: PathBuf,
    },
    /// Reading the task folders failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A git command failed.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Puts a new task on its own branch when the project is on GitHub, in a new
/// folder of its own, and returns the repository the task lives in: that
/// folder, or `repo` itself when the project is not on GitHub. `repo`'s
/// folder, its branch and its uncommitted changes stay as they are.
pub fn start(repo: &Repo, config: &Config, task_id: &str) -> Result<Repo, BranchError> {
    let branch = task_branch(task_id);
    // Even without `[github]` here (an older branch), a task with this id
    // may live on its branch: `enter` would later pick that one.
    if repo.has_branch(&branch)? {
        return Err(BranchError::Exists(branch));
    }
    if config.github.is_none() {
        return Ok(repo.clone());
    }
    // Without a commit there is nothing for the new branch to start at.
    if repo.head()?.is_none() {
        return Err(BranchError::NoCommits);
    }
    let folder = task_folder(repo, task_id)?;
    if folder.exists() {
        return Err(BranchError::FolderInTheWay(folder));
    }
    if let Err(error) = repo.add_worktree_with_branch(&folder, &branch) {
        // Git may have made the branch before it failed; take it back, so
        // the task id can be used again.
        let _ = repo.discard_new_worktree(&folder, &branch);
        return Err(error.into());
    }
    Ok(Repo::open(&folder)?)
}

/// Takes back what [`start`] made for a task whose creation then failed:
/// its new folder and branch. Does nothing for a task without a folder of
/// its own. Only for a task created a moment ago, with no work in it.
pub fn undo_start(repo: &Repo, task_id: &str) -> Result<(), BranchError> {
    let folder = task_folder(repo, task_id)?;
    if !folder.exists() {
        return Ok(());
    }
    repo.discard_new_worktree(&folder, &task_branch(task_id))?;
    Ok(())
}

/// The repository the task works in, before anything of it is read or
/// saved: the folder where its branch is checked out (the project folder
/// itself for a task begun before tasks had folders of their own), a folder
/// made again from its branch when that one was removed, or `repo` when the
/// task has no branch. A branch checked out in any other folder is refused:
/// the agents would work in a folder the user did not expect.
pub fn enter(repo: &Repo, task_id: &str) -> Result<Repo, BranchError> {
    let branch = task_branch(task_id);
    if !repo.has_branch(&branch)? {
        return Ok(repo.clone());
    }
    let expected = task_folder(repo, task_id)?;
    if let Some(folder) = repo.worktree_of(&branch)? {
        if !same_folder(&folder, &expected) && !same_folder(&folder, &main_folder(repo)?) {
            return Err(BranchError::ElsewhereFolder { branch, folder });
        }
        return Ok(Repo::open(&folder)?);
    }
    // A folder deleted by hand is still known to git, which would refuse to
    // check the branch out again until it forgets that folder.
    repo.prune_worktrees()?;
    if expected.exists() {
        return Err(BranchError::FolderInTheWay(expected));
    }
    repo.add_worktree(&expected, &branch)?;
    Ok(Repo::open(&expected)?)
}

/// Removes the task's own folder once its work is merged; the branch stays.
/// Returns `false` when there is nothing to remove: the task has no folder of
/// its own (it lives in the project folder). Git refuses while the folder
/// has uncommitted changes, so no work is lost.
pub fn finish(repo: &Repo, task_id: &str) -> Result<bool, BranchError> {
    let Some(folder) = repo.worktree_of(&task_branch(task_id))? else {
        return Ok(false);
    };
    if !same_folder(&folder, &task_folder(repo, task_id)?) {
        return Ok(false);
    }
    repo.remove_worktree(&folder)?;
    Ok(true)
}

/// Where the task's own folder goes: `<project>.tasks/<task>`, next to the
/// main folder of the repository (also when `repo` is a task's folder).
pub fn task_folder(repo: &Repo, task_id: &str) -> Result<PathBuf, BranchError> {
    let main = main_folder(repo)?;
    let Some(name) = main.file_name() else {
        return Err(BranchError::NoFolderName(main));
    };
    let mut tasks = name.to_os_string();
    tasks.push(TASKS_FOLDER_SUFFIX);
    Ok(main.with_file_name(tasks).join(task_id))
}

/// The main folder of the repository: the user's project folder, also when
/// `repo` is a task's folder. Git lists it first.
fn main_folder(repo: &Repo) -> Result<PathBuf, BranchError> {
    Ok(repo
        .worktrees()?
        .into_iter()
        .next()
        .map_or_else(|| repo.root().to_path_buf(), |tree| tree.path))
}

/// Do `a` and `b` name the same folder? Git may give a path in another
/// spelling (`/private/tmp` on macOS, `/` on Windows), so both are resolved.
fn same_folder(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// A task and the working folder it lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPlace {
    /// The task's id, for example `task-001`.
    pub id: String,
    /// The working folder whose `.harness/runs/<id>` holds the task.
    pub root: PathBuf,
}

impl TaskPlace {
    /// The folder of the task folders that holds this task.
    pub fn runs_dir(&self) -> PathBuf {
        self.root.join(HARNESS_DIR).join("runs")
    }
}

/// Every task of the project, sorted by id: those in `repo`'s own folder and
/// those in the tasks' own folders. A task found in both (merged, while its
/// folder is still there) is read from its own folder, where its newest steps are.
pub fn task_places(repo: &Repo) -> Result<Vec<TaskPlace>, BranchError> {
    let mut places: BTreeMap<String, PathBuf> = task_ids(&repo.runs_dir())?
        .into_iter()
        .map(|id| (id, repo.root().to_path_buf()))
        .collect();
    for tree in repo.worktrees()? {
        let Some(id) = tree
            .branch
            .as_deref()
            .and_then(|b| b.strip_prefix(BRANCH_PREFIX))
        else {
            continue;
        };
        let runs = tree.path.join(HARNESS_DIR).join("runs");
        // Only the folder's own task: other tasks there are older copies.
        if task_ids(&runs)?.iter().any(|found| found == id) {
            places.insert(id.to_string(), tree.path);
        }
    }
    Ok(places
        .into_iter()
        .map(|(id, root)| TaskPlace { id, root })
        .collect())
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

    /// A project in `<temp>/app`, so the tasks' folders (`<temp>/app.tasks`)
    /// are removed with the temporary folder too.
    fn project() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        fs::create_dir(&root).unwrap();
        let repo = Repo::init(&root).unwrap();
        fs::write(root.join("README.md"), "hello\n").unwrap();
        repo.commit_all("first").unwrap();
        (dir, repo)
    }

    /// A task folder `id` with a `state.json` in `runs`.
    fn add_task(runs: &Path, id: &str) {
        fs::create_dir_all(runs.join(id)).unwrap();
        fs::write(runs.join(id).join("state.json"), "{}").unwrap();
    }

    fn same(a: &Path, b: &Path) -> bool {
        a.canonicalize().unwrap() == b.canonicalize().unwrap()
    }

    #[test]
    fn a_project_on_github_starts_each_task_in_its_own_folder() {
        let (dir, repo) = project();
        let before = repo.current_branch().unwrap();
        let config = Config::parse(ON_GITHUB).unwrap();

        let task = start(&repo, &config, "task-001").unwrap();

        assert!(same(task.root(), &dir.path().join("app.tasks/task-001")));
        assert_eq!(
            task.current_branch().unwrap().as_deref(),
            Some("harness/task-001")
        );
        assert_eq!(repo.current_branch().unwrap(), before);
        let error = start(&repo, &config, "task-001").unwrap_err();
        assert!(matches!(error, BranchError::Exists(name) if name == "harness/task-001"));
    }

    #[test]
    fn uncommitted_changes_stay_in_the_project_folder() {
        let (_dir, repo) = project();
        let config = Config::parse(ON_GITHUB).unwrap();
        fs::write(repo.root().join("README.md"), "changed\n").unwrap();

        let task = start(&repo, &config, "task-001").unwrap();

        assert_eq!(
            fs::read_to_string(repo.root().join("README.md")).unwrap(),
            "changed\n"
        );
        assert_eq!(
            fs::read_to_string(task.root().join("README.md")).unwrap(),
            "hello\n"
        );
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
    fn a_project_not_on_github_keeps_its_tasks_in_its_folder() {
        let (_dir, repo) = project();
        let before = repo.current_branch().unwrap();
        let config = Config::parse("").unwrap();

        let task = start(&repo, &config, "task-001").unwrap();

        assert_eq!(task.root(), repo.root());
        assert_eq!(repo.current_branch().unwrap(), before);
        assert!(!repo.has_branch("harness/task-001").unwrap());
    }

    #[test]
    fn entering_a_task_finds_its_folder_and_the_project_stays() {
        let (_dir, repo) = project();
        let before = repo.current_branch().unwrap();
        let config = Config::parse(ON_GITHUB).unwrap();
        let started = start(&repo, &config, "task-001").unwrap();
        fs::write(repo.root().join("README.md"), "mine\n").unwrap();

        let entered = enter(&repo, "task-001").unwrap();

        assert!(same(entered.root(), started.root()));
        assert_eq!(repo.current_branch().unwrap(), before);
        assert_eq!(
            fs::read_to_string(repo.root().join("README.md")).unwrap(),
            "mine\n"
        );
    }

    #[test]
    fn a_removed_task_folder_is_made_again_from_its_branch() {
        let (_dir, repo) = project();
        let config = Config::parse(ON_GITHUB).unwrap();
        let started = start(&repo, &config, "task-001").unwrap();
        fs::write(started.root().join("parser.rs"), "fn parse() {}").unwrap();
        started.commit_all("work").unwrap();
        assert!(finish(&repo, "task-001").unwrap());
        assert!(!started.root().exists());

        let entered = enter(&repo, "task-001").unwrap();

        assert!(entered.root().join("parser.rs").is_file());
        assert!(repo.has_branch("harness/task-001").unwrap());
    }

    #[test]
    fn a_task_folder_deleted_by_hand_is_made_again() {
        let (_dir, repo) = project();
        let config = Config::parse(ON_GITHUB).unwrap();
        let started = start(&repo, &config, "task-001").unwrap();
        fs::remove_dir_all(started.root()).unwrap();

        let entered = enter(&repo, "task-001").unwrap();

        assert!(entered.root().join("README.md").is_file());
        assert!(same(
            entered.root(),
            &task_folder(&repo, "task-001").unwrap()
        ));
    }

    #[test]
    fn a_task_branch_checked_out_in_another_folder_is_refused() {
        let (dir, repo) = project();
        let base = repo.current_branch().unwrap().unwrap();
        repo.create_branch("harness/task-001").unwrap();
        repo.switch_branch(&base).unwrap();
        let other = dir.path().join("elsewhere");
        repo.add_worktree(&other, "harness/task-001").unwrap();

        let error = enter(&repo, "task-001").unwrap_err();

        assert!(
            matches!(error, BranchError::ElsewhereFolder { .. }),
            "{error}"
        );
    }

    #[test]
    fn something_in_the_way_of_a_new_task_folder_leaves_no_branch() {
        let (dir, repo) = project();
        let config = Config::parse(ON_GITHUB).unwrap();
        fs::create_dir_all(dir.path().join("app.tasks/task-001")).unwrap();

        let error = start(&repo, &config, "task-001").unwrap_err();

        assert!(matches!(error, BranchError::FolderInTheWay(_)), "{error}");
        assert!(!repo.has_branch("harness/task-001").unwrap());
    }

    #[test]
    fn undoing_a_start_removes_the_new_folder_and_branch() {
        let (_dir, repo) = project();
        let config = Config::parse(ON_GITHUB).unwrap();
        let started = start(&repo, &config, "task-001").unwrap();

        undo_start(&repo, "task-001").unwrap();

        assert!(!started.root().exists());
        assert!(!repo.has_branch("harness/task-001").unwrap());
        start(&repo, &config, "task-001").unwrap();
    }

    #[test]
    fn a_task_on_the_project_folder_branch_works_there() {
        let (_dir, repo) = project();
        repo.create_branch("harness/task-001").unwrap();

        let entered = enter(&repo, "task-001").unwrap();

        assert!(same(entered.root(), repo.root()));
        assert!(!finish(&repo, "task-001").unwrap());
        assert_eq!(
            repo.current_branch().unwrap().as_deref(),
            Some("harness/task-001")
        );
    }

    #[test]
    fn a_task_without_a_branch_stays_where_it_is() {
        let (_dir, repo) = project();
        let before = repo.current_branch().unwrap();

        let entered = enter(&repo, "task-001").unwrap();

        assert_eq!(entered.root(), repo.root());
        assert_eq!(repo.current_branch().unwrap(), before);
        assert!(!finish(&repo, "task-001").unwrap());
    }

    #[test]
    fn a_task_folder_with_uncommitted_work_is_kept() {
        let (_dir, repo) = project();
        let config = Config::parse(ON_GITHUB).unwrap();
        let started = start(&repo, &config, "task-001").unwrap();
        fs::write(started.root().join("README.md"), "unsaved\n").unwrap();

        finish(&repo, "task-001").unwrap_err();

        assert!(started.root().join("README.md").is_file());
    }

    #[test]
    fn the_tasks_folders_sit_next_to_the_main_folder() {
        let (dir, repo) = project();
        let config = Config::parse(ON_GITHUB).unwrap();
        let task = start(&repo, &config, "task-001").unwrap();

        let from_task = task_folder(&task, "task-002").unwrap();

        assert_eq!(from_task.file_name().unwrap().to_str(), Some("task-002"));
        assert!(same(
            from_task.parent().unwrap(),
            &dir.path().join("app.tasks")
        ));
    }

    #[test]
    fn the_task_list_takes_each_task_from_its_own_folder() {
        let (_dir, repo) = project();
        let config = Config::parse(ON_GITHUB).unwrap();
        add_task(&repo.runs_dir(), "task-001");
        let task = start(&repo, &config, "task-002").unwrap();
        add_task(&task.runs_dir(), "task-002");
        // An older copy of another task in a task's folder does not count.
        add_task(&task.runs_dir(), "task-009");
        // A merged task: in the project folder and still in its own folder.
        let merged = start(&repo, &config, "task-003").unwrap();
        add_task(&merged.runs_dir(), "task-003");
        add_task(&repo.runs_dir(), "task-003");

        let places = task_places(&repo).unwrap();

        let ids: Vec<&str> = places.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["task-001", "task-002", "task-003"]);
        assert_eq!(places[0].root, repo.root());
        assert!(same(&places[1].root, task.root()));
        assert!(same(&places[2].root, merged.root()));
        assert_eq!(places[1].runs_dir(), places[1].root.join(".harness/runs"));
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
