//! Task ids: listing them, choosing the next one, and keeping them safe to
//! use as folder names.

use std::path::Path;

use super::files::sorted_subdirs;
use super::{StoreError, STATE_FILE};
use crate::task::handoff::Role;

/// The ids of all tasks in `runs_dir` (folders with a `state.json`), sorted.
pub fn task_ids(runs_dir: &Path) -> Result<Vec<String>, StoreError> {
    if !runs_dir.exists() {
        return Ok(Vec::new());
    }
    Ok(sorted_subdirs(runs_dir)?
        .into_iter()
        .filter(|dir| dir.join(STATE_FILE).exists())
        .filter_map(|dir| Some(dir.file_name()?.to_string_lossy().into_owned()))
        .filter(|id| check_task_id(id).is_ok())
        .collect())
}

/// The id for the next new task: `task-001`, or one more than the highest
/// `task-NNN` in `runs_dir`.
pub fn next_task_id(runs_dir: &Path) -> Result<String, StoreError> {
    let highest = task_ids(runs_dir)?
        .iter()
        .filter_map(|id| id.strip_prefix("task-")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    Ok(format!("task-{:03}", highest + 1))
}

/// `round-02-tester-1.log` -> (2, Tester).
pub(super) fn parse_failure(name: &str) -> Option<(u32, Role)> {
    let rest = name.strip_prefix("round-")?.strip_suffix(".log")?;
    let (round, rest) = rest.split_once('-')?;
    let (role, attempt) = rest.rsplit_once('-')?;
    attempt.parse::<u32>().ok()?;
    let role = role.parse().ok()?;
    Some((round.parse().ok()?, role))
}

/// Task ids become folder names, so we allow only safe characters.
/// This blocks tricks like `../../etc`.
pub(super) fn check_task_id(task_id: &str) -> Result<(), StoreError> {
    let ok = !task_id.is_empty()
        && task_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(StoreError::InvalidTaskId(task_id.to_string()))
    }
}
