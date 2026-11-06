//! What is going on in a project at a glance, for the main TUI window's list
//! of projects: which task a harness runs there now (perhaps in another
//! window) and how many tasks wait for the user.
//!
//! Everything is read from disk and git each time, never from one window's
//! memory, so the main window also sees the work of windows pinned to a
//! project and of `harness run` in a terminal.

use crate::git::Repo;
use crate::task::run_lock::{self, RunLockError, Working};
use crate::task::store::{self, StoreError, TaskStore};
use crate::task::Stage;

/// A project's state at a glance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overview {
    /// The task a harness works on there right now, if any.
    pub working: Option<Working>,
    /// The tasks that wait for the user (a design to approve, a question,
    /// a result to accept), in id order.
    pub waiting: Vec<String>,
}

/// Why a project's overview could not be read.
#[derive(Debug, thiserror::Error)]
pub enum OverviewError {
    /// A task's files could not be read.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The run lock could not be checked.
    #[error(transparent)]
    RunLock(#[from] RunLockError),
}

/// Reads the overview of the project in `repo`. Changes nothing: a running
/// harness is never disturbed (see [`run_lock::working`]).
///
/// # Errors
///
/// A task's files cannot be read, or the run lock cannot be checked.
pub fn overview(repo: &Repo) -> Result<Overview, OverviewError> {
    let working = run_lock::working(repo)?;
    let runs = repo.runs_dir();
    let mut waiting = Vec::new();
    for id in store::task_ids(&runs)? {
        let (_store, state) = TaskStore::open(&runs, &id)?;
        if matches!(state.stage, Stage::WaitingForHuman(_)) {
            waiting.push(id);
        }
    }
    Ok(Overview { working, waiting })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::projects;
    use crate::task::handoff::{Handoff, NextStep, Role, Verdict};
    use crate::task::orchestrator;
    use crate::task::run_lock::RunLock;
    use crate::task::scenario::Workflow;

    fn designed(repo: &Repo, task: &str) {
        let (store, mut state) =
            orchestrator::create_task(repo, task, &format!("Build {task}"), Workflow::Task, 5)
                .unwrap();
        let design = Handoff {
            schema_version: 1,
            task_id: task.into(),
            round: 1,
            role: Role::Architect,
            verdict: Verdict::Approved,
            next_role: NextStep::To(Role::Human),
            summary: "Design ready".into(),
            skills_used: vec![],
            files: vec![],
            issues: vec![],
            answer_to: None,
        };
        store.record(&mut state, &design, "notes").unwrap();
    }

    #[test]
    fn an_empty_project_has_nothing_going_on() {
        let dir = tempfile::tempdir().unwrap();
        projects::init(dir.path()).unwrap();
        let repo = Repo::open(dir.path()).unwrap();

        assert_eq!(overview(&repo).unwrap(), Overview::default());
    }

    #[test]
    fn waiting_tasks_and_the_running_one_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        projects::init(dir.path()).unwrap();
        let repo = Repo::open(dir.path()).unwrap();
        designed(&repo, "task-001");
        orchestrator::create_task(&repo, "task-002", "Other", Workflow::Task, 5).unwrap();
        designed(&repo, "task-003");
        // Another window runs task-002.
        let mut lock = RunLock::hold(&repo).unwrap();
        lock.set_task("task-002").unwrap();

        let seen = overview(&repo).unwrap();

        assert_eq!(seen.waiting, ["task-001", "task-003"]);
        assert_eq!(seen.working.map(|w| w.task).as_deref(), Some("task-002"));
    }
}
