//! Creating a task, and the guard against creating the same task twice.

use std::path::Path;

use super::RunError;
use crate::git::Repo;
use crate::task::store::{self, StoreError, TaskStore};
use crate::task::{Stage, TaskState};

/// Creates a new task in `<project>/.harness/runs/` and commits it. If an
/// unfinished task already has the same text (a message sent twice), nothing
/// is created and the error names that task.
pub fn create_task(
    repo: &Repo,
    task_id: &str,
    description: &str,
    max_rounds: u32,
) -> Result<(TaskStore, TaskState), RunError> {
    if let Some((task, stage)) = same_task(&repo.runs_dir(), description)? {
        return Err(RunError::SameTask {
            task,
            stage: crate::retro::stage_text(stage),
        });
    }
    create_task_anyway(repo, task_id, description, max_rounds)
}

/// Like [`create_task`], but also when an unfinished task has the same text.
pub fn create_task_anyway(
    repo: &Repo,
    task_id: &str,
    description: &str,
    max_rounds: u32,
) -> Result<(TaskStore, TaskState), RunError> {
    repo.ensure_harness_ignores()?;
    let runs = repo.runs_dir();
    std::fs::create_dir_all(&runs).map_err(|source| StoreError::Io {
        path: runs.clone(),
        source,
    })?;
    let (store, state) = TaskStore::create(&runs, task_id, description, max_rounds)?;
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
