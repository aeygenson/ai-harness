//! The user's decisions, saved as a handoff with role `human`.

use std::path::{Path, PathBuf};

use super::{commit_message, RunError};
use crate::git::Repo;
use crate::task::handoff::{Handoff, NextStep, Role, Verdict};
use crate::task::rethink::DEFAULT_RETHINK_NOTE;
use crate::task::routes::ArchitectJob;
use crate::task::store::{StoreError, TaskStore};
use crate::task::{Stage, TaskState, WaitReason, DEFAULT_MAX_ROUNDS};

/// What the user decided on a waiting task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HumanDecision<'a> {
    /// Approve, send back or answer.
    pub verdict: Verdict,
    /// Who continues, or `Done`.
    pub next: NextStep,
    /// The user's notes for the next role; the first line is the summary.
    pub notes: &'a str,
    /// First make the code as it was after this step (its folder in
    /// `runs/`), with its own commit; `None` leaves the code as it is.
    pub restore_to: Option<&'a Path>,
}

/// The user asks the architect to rethink the approach of a task that waits
/// for them (see [`crate::task::rethink`]), with `notes` for the architect.
///
/// Saved like any decision of the user. Where the user has not accepted
/// the work (a design, a result, a new approach) it is a rejection, as
/// sending that work to a role is in the TUI; at the round limit the task
/// first gets the rounds a new task gets, so the answer can go on.
///
/// # Errors
/// As [`record_human_decision`]: a task that does not wait for the user
/// refuses it and nothing changes. A rejection is refused
/// ([`RunError::UnfinishedWork`]) while a stopped role's unfinished work is
/// still in the folder: a new round could not continue it.
pub fn ask_for_rethink(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
    notes: &str,
) -> Result<PathBuf, RunError> {
    let Stage::WaitingForHuman(reason) = state.stage else {
        return Err(RunError::NotWaiting);
    };
    let verdict = match reason {
        WaitReason::ApproveDesign | WaitReason::AcceptResult | WaitReason::ApproveNewApproach => {
            Verdict::Rejected
        }
        WaitReason::RoleAskedForHelp(_)
        | WaitReason::RoundLimitReached
        | WaitReason::StoppedByUser(_) => Verdict::Approved,
    };
    if verdict == Verdict::Rejected {
        let files = super::changes::unfinished_left(repo, store)?;
        if !files.is_empty() {
            return Err(RunError::UnfinishedWork { files });
        }
    }
    if reason == WaitReason::RoundLimitReached {
        // Saved first, so the decision's commit carries it (as «more rounds»).
        store.add_rounds(state, DEFAULT_MAX_ROUNDS)?;
    }
    let notes = if notes.trim().is_empty() {
        DEFAULT_RETHINK_NOTE
    } else {
        notes
    };
    // The user's decision to the architect keeps a rethink job (see
    // `TaskState::next_architect_job`). If the decision is refused before
    // it is saved, the stage is unchanged and the old job comes back.
    let old_job = state.architect_job;
    let waiting = state.stage;
    state.architect_job = ArchitectJob::Rethink;
    let decision = HumanDecision {
        verdict,
        next: NextStep::To(Role::Architect),
        notes,
        restore_to: None,
    };
    let result = record_human_decision(repo, store, state, decision);
    if result.is_err() && state.stage == waiting {
        state.architect_job = old_job;
    }
    result
}

/// The user's decision (approve the design, send work back, answer a question),
/// saved like any other handoff with role `human`.
///
/// With `restore_to`, the code is first restored to that step (see
/// `restore.rs`) and the notes say so to the next role.
///
/// Finishing the task is refused ([`RunError::UnfinishedWork`]) while the
/// unfinished work a limit or «Stop now» left is still uncommitted: it
/// would be left behind in the folder of a finished task.
pub fn record_human_decision(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
    decision: HumanDecision<'_>,
) -> Result<PathBuf, RunError> {
    let HumanDecision {
        verdict,
        next,
        notes,
        restore_to,
    } = decision;
    if next == NextStep::Done {
        let files = super::changes::unfinished_left(repo, store)?;
        if !files.is_empty() {
            return Err(RunError::UnfinishedWork { files });
        }
    }
    // The summary is the first line of the user's own notes.
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
    let notes = match restore_to {
        Some(step_dir) => {
            // Check the decision on a copy first: a refused one (a wrong
            // route, a task that does not wait for the user) must not leave
            // a restored-code commit behind.
            state.clone().apply(&handoff).map_err(StoreError::from)?;
            let line = super::restore::restore_code(repo, state, step_dir)?;
            if notes.trim().is_empty() {
                line
            } else {
                format!("{}\n\n{line}", notes.trim_end())
            }
        }
        None => notes.to_string(),
    };
    let notes = notes.as_str();
    let step_dir = store.record(state, &handoff, notes)?;
    // Commit only the task folder: code the user changed themselves is theirs to commit.
    repo.commit_paths(&[store.dir()], &commit_message(&handoff))?;
    Ok(step_dir)
}
