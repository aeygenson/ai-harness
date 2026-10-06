//! Lisa's decisions, saved as a handoff with role `human`.

use std::path::PathBuf;

use super::{commit_message, RunError};
use crate::git::Repo;
use crate::task::handoff::{Handoff, NextStep, Role, Verdict};
use crate::task::store::TaskStore;
use crate::task::TaskState;

/// Lisa's decision (approve the design, send work back, answer a question),
/// saved like any other handoff with role `human`.
pub fn record_human_decision(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
    verdict: Verdict,
    next: NextStep,
    notes: &str,
) -> Result<PathBuf, RunError> {
    let summary = notes
        .lines()
        .next()
        .filter(|line| !line.trim().is_empty())
        .unwrap_or("Lisa's decision")
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
    };
    let step_dir = store.record(state, &handoff, notes)?;
    // Commit only the task folder: code Lisa changed herself is hers to commit.
    repo.commit_paths(&[store.dir()], &commit_message(&handoff))?;
    Ok(step_dir)
}
