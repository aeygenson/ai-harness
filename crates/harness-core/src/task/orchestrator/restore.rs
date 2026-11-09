//! «Restore the code to a step»: the user's decision can first make the
//! project's code as it was right after an earlier step, with a new commit.
//! Nothing is rewritten: later commits and the task's history in `.harness/`
//! stay, so the restore itself can be undone with git.

use std::path::Path;

use super::RunError;
use crate::git::Repo;
use crate::task::TaskState;

/// Makes the code outside `.harness/` as it was after the step in `step_dir`
/// (a folder such as `runs/task-001/round-01/01-architect`) and commits it.
/// Returns the line for the next role's notes that says so.
///
/// Only what the restore itself changed is committed: an ignored file such
/// as `.env` stays out of git even when an older `.gitignore` comes back.
///
/// # Errors
/// [`RunError::NotTopLevel`] for a project inside a larger repository,
/// [`RunError::NotClean`] while the folder has uncommitted changes (they
/// would be lost or mixed in), [`RunError::NoStepCommit`] when the step is
/// not in the branch's history, [`RunError::IgnoredInTheWay`] when ignored
/// files would be written over, and git errors.
pub(super) fn restore_code(
    repo: &Repo,
    state: &TaskState,
    step_dir: &Path,
) -> Result<String, RunError> {
    if !repo.is_top_level()? {
        return Err(RunError::NotTopLevel);
    }
    let files = repo.changed_files()?;
    if !files.is_empty() {
        return Err(RunError::NotClean { files });
    }
    let step = step_label(step_dir);
    let Some(handoff) = git_path(repo, &step_dir.join("handoff.json")) else {
        return Err(RunError::NoStepCommit { step });
    };
    let Some(commit) = repo.find_commit_adding(&handoff)? else {
        return Err(RunError::NoStepCommit { step });
    };
    let files = repo.ignored_in_the_way(&commit)?;
    if !files.is_empty() {
        return Err(RunError::IgnoredInTheWay { files });
    }
    repo.restore_project_from(&commit)?;
    let message = format!(
        "{} round {}: code restored to {step} by the user",
        state.task_id, state.round
    );
    let short = commit.get(..7).unwrap_or(&commit);
    if !repo.commit_staged(&message)? {
        return Ok(format!(
            "The user asked to restore the code to how it was after step {step} \
             (commit {short}); it already was, so nothing changed."
        ));
    }
    Ok(format!(
        "The harness restored the code outside .harness/ to how it was after step \
         {step} (commit {short}); later changes to the code were undone."
    ))
}

/// The step as the user sees it: `round-01/01-architect`.
fn step_label(step_dir: &Path) -> String {
    let mut parts = step_dir.iter().rev().take(2).collect::<Vec<_>>();
    parts.reverse();
    parts
        .iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// `path` relative to the project, with `/`, as git names it; `None` when it
/// is not inside the project.
fn git_path(repo: &Repo, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(repo.root()).ok()?;
    Some(harness_platform::path::slashed(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_is_named_by_its_round_and_folder() {
        let dir = Path::new("/p/.harness/runs/task-001/round-02/03-tester");

        assert_eq!(step_label(dir), "round-02/03-tester");
    }
}
