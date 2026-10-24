//! The user's decisions, saved as a handoff with role `human`.

use std::path::PathBuf;

use super::{commit_message, RunError};
use crate::git::Repo;
use crate::task::handoff::{Handoff, NextStep, Role, Verdict};
use crate::task::store::TaskStore;
use crate::task::TaskState;

/// The user's decision (approve the design, send work back, answer a question),
/// saved like any other handoff with role `human`.
///
/// Finishing the task is refused ([`RunError::UnfinishedWork`]) while the
/// unfinished work a limit or «Stop now» left is still uncommitted: it
/// would be left behind in the folder of a finished task.
pub fn record_human_decision(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
    verdict: Verdict,
    next: NextStep,
    notes: &str,
) -> Result<PathBuf, RunError> {
    if next == NextStep::Done {
        let files = super::changes::unfinished_left(repo, store)?;
        if !files.is_empty() {
            return Err(RunError::UnfinishedWork { files });
        }
    }
    let summary = notes
        .lines()
        .next()
        .filter(|line| !line.trim().is_empty())
        .unwrap_or("The user's decision")
        .to_string();
    let issues = if verdict == Verdict::Rejected {
        vec![crate::task::handoff::Issue {
            severity: crate::task::handoff::Severity::Medium,
            location: None,
            description: summary.clone(),
        }]
    } else {
        vec![]
    };
    let handoff = Handoff {
        schema_version: crate::task::handoff::SCHEMA_VERSION,
        task_id: state.task_id.clone(),
        round: state.round,
        role: Role::Human,
        verdict,
        next_role: next,
        summary,
        skills_used: vec![],
        files: vec![],
        issues,
        answer_to: None,
    };
    let step_dir = store.record(state, &handoff, notes)?;
    // Commit only the task folder: code the user changed themselves is theirs to commit.
    repo.commit_paths(&[store.dir()], &commit_message(&handoff))?;
    Ok(step_dir)
}
