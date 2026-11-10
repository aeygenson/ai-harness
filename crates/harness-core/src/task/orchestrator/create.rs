//! Creating a task, and the guard against creating the same task twice.

use super::RunError;
use crate::config::Config;
use crate::git::Repo;
use crate::task::branch;
use crate::task::scenario::Workflow;
use crate::task::store::{StoreError, TaskStore};
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
/// round limit, and in a project on GitHub its own branch and folder (see
/// [`branch::start`]), where its files are saved and committed.
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
    // Checked and committed before the task's branch and folder are made: a
    // wrong id then makes nothing, and the harness's own setup is committed on
    // the project's branch, so the new folder starts with it.
    TaskStore::check_new(&repo.runs_dir(), task_id)?;
    repo.ensure_harness_ignores()?;
    let task_repo = branch::start(repo, config, task_id)?;
    let created = create_task_anyway(
        &task_repo,
        task_id,
        description,
        config.scenario.default_workflow(),
        config.max_rounds,
    );
    if created.is_err() {
        // A half-made task folder and branch would block this id; the
        // error to show is the one above, so this one is dropped.
        let _ = branch::undo_start(repo, task_id);
    }
    created
}

/// An error naming the unfinished task that already has this text, if any,
/// in the project folder or in a task's own folder.
fn refuse_same_text(repo: &Repo, description: &str) -> Result<(), RunError> {
    match same_task(repo, description)? {
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
pub fn same_task(repo: &Repo, description: &str) -> Result<Option<(String, Stage)>, RunError> {
    let wanted = words(description);
    for place in branch::task_places(repo)? {
        // A task that cannot be read is no reason to refuse a new one.
        let Ok((task, state)) = TaskStore::open(&place.runs_dir(), &place.id) else {
            continue;
        };
        let Ok(text) = task.description() else {
            continue;
        };
        if state.stage != Stage::Done && words(&text) == wanted {
            return Ok(Some((place.id, state.stage)));
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

    /// A project in `<temp>/app`: the tasks' own folders go to `<temp>/app.tasks`.
    fn project() -> (tempfile::TempDir, Repo, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        fs::create_dir(&root).unwrap();
        let repo = Repo::init(&root).unwrap();
        fs::write(root.join("README.md"), "hello\n").unwrap();
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
        let task = branch::enter(&repo, "task-001").unwrap();
        assert_eq!(
            task.current_branch().unwrap(),
            Some(task_branch("task-001"))
        );
        assert_ne!(task.head().unwrap(), base_commit);
        assert!(task.runs_dir().join("task-001/state.json").is_file());
        // The project folder stays on the base branch, with nothing of the task.
        assert_eq!(repo.current_branch().unwrap(), Some(base));
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
        assert_eq!(repo.current_branch().unwrap(), Some(base));
    }

    #[test]
    fn a_task_that_fails_in_its_new_folder_leaves_no_folder_or_branch() {
        let (dir, repo, _base) = project();
        let config = Config::parse(ON_GITHUB).unwrap();
        // A task committed here but deleted by hand (not committed): the new
        // folder, made from the last commit, has it, so creating it there fails.
        let old = repo.runs_dir().join("task-001");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("state.json"), "{}").unwrap();
        repo.commit_all("an old task").unwrap();
        fs::remove_dir_all(&old).unwrap();

        let error = create_project_task(&repo, &config, "task-001", "Add", SameText::Allow);

        assert!(matches!(error, Err(RunError::Store(_))), "{error:?}");
        assert!(!dir.path().join("app.tasks/task-001").exists());
        assert!(!repo.has_branch(&task_branch("task-001")).unwrap());
    }

    #[test]
    fn the_same_text_is_allowed_when_asked() {
        let (_dir, repo, _base) = project();
        let config = Config::parse("").unwrap();
        create_project_task(&repo, &config, "task-001", "Add a parser", SameText::Refuse).unwrap();

        create_project_task(&repo, &config, "task-002", "Add a parser", SameText::Allow).unwrap();

        assert!(repo.runs_dir().join("task-002").exists());
        assert_eq!(
            repo.branches_starting_with("harness/").unwrap(),
            Vec::<String>::new()
        );
    }
}
