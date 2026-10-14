//! Creating a task, and the guard against creating the same task twice.

use std::path::Path;

use super::RunError;
use crate::config::Config;
use crate::git::Repo;
use crate::task::branch;
use crate::task::scenario::Workflow;
use crate::task::store::{self, StoreError, TaskStore};
use crate::task::{Stage, TaskState};

/// Creates a new task in `<project>/.harness/runs/` and commits it.
///
/// The task follows `workflow` (see `Scenario::default_workflow`). If an
/// unfinished task already has the same text (a message sent twice), nothing
/// is created and the error names that task.
pub fn create_task(
    repo: &Repo,
    task_id: &str,
    description: &str,
    workflow: Workflow,
    max_rounds: u32,
) -> Result<(TaskStore, TaskState), RunError> {
    refuse_same_text(repo, description)?;
    create_task_anyway(repo, task_id, description, workflow, max_rounds)
}

/// What to do when an unfinished task already has the same text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameText {
    /// Create nothing and name that task (usually the text was sent twice).
    Refuse,
    /// Create the task anyway.
    Allow,
}

/// Creates a task with the project's settings: the scenario's workflow, the
/// round limit, and in a project on GitHub its own branch (see
/// [`branch::start`]), which is checked out from then on.
pub fn create_project_task(
    repo: &Repo,
    config: &Config,
    task_id: &str,
    description: &str,
    same_text: SameText,
) -> Result<(TaskStore, TaskState), RunError> {
    if same_text == SameText::Refuse {
        refuse_same_text(repo, description)?;
    }
    // Checked and committed before the branch is made, so a wrong id leaves
    // no empty branch and the harness's own setup stays on the current one.
    TaskStore::check_new(&repo.runs_dir(), task_id)?;
    repo.ensure_harness_ignores()?;
    branch::start(repo, config, task_id)?;
    create_task_anyway(
        repo,
        task_id,
        description,
        config.scenario.default_workflow(),
        config.max_rounds,
    )
}

/// An error naming the unfinished task that already has this text, if any.
fn refuse_same_text(repo: &Repo, description: &str) -> Result<(), RunError> {
    match same_task(&repo.runs_dir(), description)? {
        Some((task, stage)) => Err(RunError::SameTask {
            task,
            stage: crate::retro::stage_text(stage),
        }),
        None => Ok(()),
    }
}

/// Like [`create_task`], but also when an unfinished task has the same text.
pub fn create_task_anyway(
    repo: &Repo,
    task_id: &str,
    description: &str,
    workflow: Workflow,
    max_rounds: u32,
) -> Result<(TaskStore, TaskState), RunError> {
    repo.ensure_harness_ignores()?;
    let runs = repo.runs_dir();
    std::fs::create_dir_all(&runs).map_err(|source| StoreError::Io {
        path: runs.clone(),
        source,
    })?;
    let (store, state) = TaskStore::create(&runs, task_id, description, workflow, max_rounds)?;
    repo.commit_paths(&[store.dir()], &format!("{task_id}: new task"))?;
    Ok((store, state))
}

/// The unfinished task whose text is `description`, and its stage. Spaces
/// and line breaks do not count: a pasted copy of the same text matches.
pub fn same_task(runs: &Path, description: &str) -> Result<Option<(String, Stage)>, StoreError> {
    if !runs.is_dir() {
        return Ok(None);
    }
    let wanted = words(description);
    for id in store::task_ids(runs)? {
        // A task that cannot be read is no reason to refuse a new one.
        let Ok((task, state)) = TaskStore::open(runs, &id) else {
            continue;
        };
        let Ok(text) = task.description() else {
            continue;
        };
        if state.stage != Stage::Done && words(&text) == wanted {
            return Ok(Some((id, state.stage)));
        }
    }
    Ok(None)
}

/// The words of `text`, so that only spaces and line breaks may differ.
pub fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::task::branch::task_branch;

    const ON_GITHUB: &str = "[github]\nrepo = \"lisa/app\"\n";

    fn project() -> (tempfile::TempDir, Repo, String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        fs::write(dir.path().join("README.md"), "hello\n").unwrap();
        repo.commit_all("first").unwrap();
        // As after `harness init`, so creating a task commits nothing on the base.
        repo.ensure_harness_ignores().unwrap();
        let base = repo.current_branch().unwrap().unwrap();
        (dir, repo, base)
    }

    #[test]
    fn a_task_of_a_project_on_github_is_committed_on_its_own_branch() {
        let (_dir, repo, base) = project();
        let base_commit = repo.head().unwrap();
        let config = Config::parse(ON_GITHUB).unwrap();

        let (_, state) =
            create_project_task(&repo, &config, "task-001", "Add a parser", SameText::Refuse)
                .unwrap();

        assert_eq!(state.max_rounds, config.max_rounds);
        assert_eq!(
            repo.current_branch().unwrap(),
            Some(task_branch("task-001"))
        );
        assert_ne!(repo.head().unwrap(), base_commit);
        // The base branch has nothing of the task.
        repo.switch_branch(&base).unwrap();
        assert_eq!(repo.head().unwrap(), base_commit);
        assert!(!repo.runs_dir().join("task-001").exists());
    }

    #[test]
    fn a_task_that_cannot_be_created_leaves_no_branch() {
        let (_dir, repo, base) = project();
        let config = Config::parse(ON_GITHUB).unwrap();
        create_project_task(&repo, &config, "task-001", "Add a parser", SameText::Refuse).unwrap();

        // The same text again, and an id that is not a safe folder name.
        let same =
            create_project_task(&repo, &config, "task-002", "Add a parser", SameText::Refuse);
        let wrong_id = create_project_task(&repo, &config, "Task 2", "Other", SameText::Refuse);

        assert!(matches!(same, Err(RunError::SameTask { .. })));
        assert!(matches!(wrong_id, Err(RunError::Store(_))));
        assert_eq!(
            repo.branches_starting_with("harness/").unwrap(),
            ["harness/task-001"]
        );
        assert_eq!(
            repo.current_branch().unwrap(),
            Some(task_branch("task-001"))
        );
        assert_ne!(base, task_branch("task-001"));
    }

    #[test]
    fn the_same_text_is_allowed_when_asked() {
        let (_dir, repo, _base) = project();
        let config = Config::parse("").unwrap();
        create_project_task(&repo, &config, "task-001", "Add a parser", SameText::Refuse).unwrap();

        create_project_task(&repo, &config, "task-002", "Add a parser", SameText::Allow).unwrap();

        assert!(repo.runs_dir().join("task-002").exists());
        assert!(repo.branches_starting_with("harness/").unwrap().is_empty());
    }
}
