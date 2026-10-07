//! Putting back the files no role may change (see
//! [`permissions::is_protected`]) right after an agent finishes, before the
//! harness or the next agent reads them.

use std::path::PathBuf;

use super::{commit_failure_logs, RunError, StopReason};
use crate::git::Repo;
use crate::task::handoff::Role;
use crate::task::permissions;
use crate::task::store::TaskStore;
use crate::task::TaskState;

/// If the agent changed protected files, puts them back as they are in the
/// last commit and saves what it wrote there as a failure log, committed
/// together with the role's other failure logs. Returns the reason to stop,
/// or `None` when no protected file was touched.
pub(super) fn put_back_protected(
    repo: &Repo,
    store: &TaskStore,
    state: &TaskState,
    role: Role,
    failure_logs: &mut Vec<PathBuf>,
) -> Result<Option<StopReason>, RunError> {
    // The failure logs are the harness's own files, not the agent's changes.
    let files: Vec<String> = repo
        .changed_files()?
        .into_iter()
        .filter(|path| !failure_logs.contains(&repo.root().join(path)))
        .filter(|path| permissions::is_protected(path))
        .collect();
    if files.is_empty() {
        return Ok(None);
    }
    let shown = format!(
        "The {role} changed protected files. The harness put them back; \
         this is what the agent wrote:\n\n{}",
        repo.describe_changes(&files)?
    );
    repo.restore(&files)?;
    let log = store.save_failure_log(state.round, role, &shown)?;
    failure_logs.push(log.clone());
    commit_failure_logs(repo, failure_logs, state, role, "changed protected files")?;
    Ok(Some(StopReason::ProtectedFilesChanged { role, files, log }))
}
