//! The user's stop buttons: the task stops and waits for them
//! ([`WaitReason::StoppedByUser`]), and «Continue» goes on from there.
//!
//! - «Stop after this step»: the running role finishes and its outcome is
//!   handled as usual; the run stops before the next role starts.
//! - «Stop now»: the agent is stopped at once (`harness_agents::process`)
//!   and its unfinished changes are kept exactly as after a time limit
//!   (see [`super::changes`]), so the next run of the same role in the same
//!   round continues from them.
//!
//! While the task waits, the user sends a note to any role, or continues. A note
//! to another role than the stopped one leaves the stopped role's unfinished
//! changes and their record as they are: that other role then stops on the
//! uncommitted changes (`DirtyWorkingTree`), as after a time limit, and the user
//! commits or removes them themselves.

use std::path::PathBuf;

use super::{RunError, StopReason};
use crate::git::Repo;
use crate::task::agent::{AgentOutcome, RunEnd};
use crate::task::handoff::Role;
use crate::task::stop_request::{StopRequest, StopWhen};
use crate::task::store::{StoreError, TaskStore};
use crate::task::{Stage, TaskState, WaitReason};

/// Stops before `role` starts if the user asked: «Stop now» always, «Stop after
/// this step» only once a step of this run was accepted (`step_done`).
pub(super) fn stop_before(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
    role: Role,
    stop: &StopRequest,
    step_done: bool,
) -> Result<Option<StopReason>, RunError> {
    let wanted = match stop.asked() {
        Some(StopWhen::Now) => true,
        Some(StopWhen::AfterStep) => step_done,
        None => false,
    };
    if !wanted {
        return Ok(None);
    }
    wait_for_user(repo, store, state, role).map(Some)
}

/// The task waits for the user, with `role` the one that continues; only
/// `state.json` is committed (code is never committed here).
pub(super) fn wait_for_user(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
    role: Role,
) -> Result<StopReason, RunError> {
    let reason = WaitReason::StoppedByUser(role);
    store.save_stage(state, Stage::WaitingForHuman(reason))?;
    let message = format!(
        "{} round {}: stopped by the user, {role} next",
        state.task_id, state.round
    );
    repo.commit_paths(&[&store.state_file()], &message)?;
    Ok(StopReason::WaitingForHuman(reason))
}

/// «Continue» on a task the user stopped: the role they stopped (or the one that
/// was next) works again, continuing from its recorded unfinished work.
/// Returns `false` and changes nothing when the task was not stopped by the user.
///
/// Only `state.json` is committed, inside `.harness/`, so the unfinished
/// work is still resumed (see `changes::resumable`).
pub fn continue_stopped(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
) -> Result<bool, RunError> {
    let Stage::WaitingForHuman(WaitReason::StoppedByUser(role)) = state.stage else {
        return Ok(false);
    };
    store.save_stage(state, Stage::Working(role))?;
    let message = format!(
        "{} round {}: the user continues, {role} next",
        state.task_id, state.round
    );
    repo.commit_paths(&[&store.state_file()], &message)?;
    Ok(true)
}

/// Saves the log of a run a limit or the user stopped: the user's stop is not a
/// failed attempt, so its log goes to a folder of its own.
pub(super) fn save_stop_log(
    store: &TaskStore,
    state: &TaskState,
    role: Role,
    outcome: &AgentOutcome,
) -> Result<PathBuf, StoreError> {
    if outcome.end == RunEnd::StoppedByUser {
        store.save_stopped_log(state.round, role, &outcome.log)
    } else {
        store.save_failure_log(state.round, role, &outcome.log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::scenario::Workflow;

    fn setup() -> (tempfile::TempDir, Repo, TaskStore, TaskState) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        repo.commit_all("first").unwrap();
        let (store, state) =
            super::super::create_task(&repo, "task-001", "Do it.", Workflow::Task, 5).unwrap();
        (dir, repo, store, state)
    }

    #[test]
    fn nothing_asked_or_no_step_done_does_not_stop() {
        let (_dir, repo, store, mut state) = setup();
        let stop = StopRequest::new();
        let before = state.clone();

        let none = stop_before(&repo, &store, &mut state, Role::Architect, &stop, true).unwrap();
        stop.toggle_after_step();
        let first = stop_before(&repo, &store, &mut state, Role::Architect, &stop, false).unwrap();

        assert_eq!(none, None);
        assert_eq!(first, None, "the running role finishes its step first");
        assert_eq!(state, before);
    }

    #[test]
    fn a_stop_waits_for_the_user_and_continue_goes_back_to_the_role() {
        let (_dir, repo, store, mut state) = setup();
        let stop = StopRequest::new();
        stop.stop_now();

        let stopped =
            stop_before(&repo, &store, &mut state, Role::Architect, &stop, false).unwrap();

        let waiting = Stage::WaitingForHuman(WaitReason::StoppedByUser(Role::Architect));
        assert_eq!(
            stopped,
            Some(StopReason::WaitingForHuman(WaitReason::StoppedByUser(
                Role::Architect
            )))
        );
        assert_eq!(state.stage, waiting);
        assert_eq!(
            TaskStore::open(&repo.runs_dir(), "task-001").unwrap().1,
            state
        );
        assert_eq!(
            repo.changed_files().unwrap(),
            Vec::<String>::new(),
            "state.json is committed"
        );

        assert!(continue_stopped(&repo, &store, &mut state).unwrap());
        assert_eq!(state.stage, Stage::Working(Role::Architect));
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
        // A task in another stage is left alone.
        assert!(!continue_stopped(&repo, &store, &mut state).unwrap());
    }
}
