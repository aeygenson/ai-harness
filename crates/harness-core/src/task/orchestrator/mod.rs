//! The loop that runs roles one after another until the task has to stop.
//!
//! Around every role the harness uses git:
//!
//! 1. Before: the project must have no uncommitted changes, so everything that
//!    changes afterwards was done by this role. Stub lines an unwatched run of
//!    Claude Code left in `.git/info/exclude` are removed first, so they hide
//!    nothing (see [`crate::task::agent_stubs`]).
//! 2. After: git's hooks and a few files outside the project are compared
//!    with before (see [`crate::task::tripwire`]), and the empty stub files
//!    Claude Code made are removed; files no role may change
//!    (agent instructions and settings, the harness's own files) are put back
//!    at once; then the changed files are
//!    checked against the role's permissions and against [`MAX_CHANGE_BYTES`],
//!    so build output never lands in git.
//! 3. If the role's work is accepted, everything is committed:
//!    `task-001 round 2: tester (rejected) - ...`. A failed attempt is thrown away.

mod changes;
mod create;
mod guard;
mod human;
mod job;
mod protect;
mod size;
mod stop;

pub use create::{
    create_project_task, create_task, create_task_anyway, same_task, words, SameText,
};
pub use human::record_human_decision;
pub use size::oversized_changes;
pub use stop::{RunError, StopReason};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{AgentKind, Config};
use crate::git::{GitError, Repo};
use crate::skills::Skills;
use crate::task::agent::{AgentRunner, RunEnd};
use crate::task::agent_stubs;
use crate::task::handoff::{Handoff, Role, Verdict};
use crate::task::manifest::AgentFacts;
use crate::task::store::{StoreError, TaskStore};
use crate::task::tripwire::Watched;
use crate::task::{Stage, TaskState};
use crate::text;
use changes::{after_failure, check_changes, stop_at_limit, AfterFailure, RoleAt};
use guard::Before;
use job::prepare_job;

/// How many times one role may try before the harness gives up and asks Lisa.
pub const ATTEMPTS_PER_ROLE: u32 = 2;

/// A role's change bigger than this (one file, or the new files of one folder)
/// is not committed. It is almost always build output or downloaded packages
/// (`target/`, `node_modules/`, `bin/`) that belong in `.gitignore`, whatever
/// the project is written in.
pub const MAX_CHANGE_BYTES: u64 = 50 * 1024 * 1024;

/// Safety net: never run more than this many roles in one call to `run`.
pub const MAX_STEPS_PER_RUN: u32 = 50;

/// Runs roles until the task is done or someone has to look at it.
///
/// Returns `Err` only for real problems of the harness itself (for example the
/// disk is full). Mistakes of an agent are a `StopReason`, not an error.
pub async fn run<A: AgentRunner>(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
    agent: &A,
) -> Result<StopReason, RunError> {
    let watched = Watched::for_repo(repo)?;
    let setup = RunSetup {
        skills: &Skills::none(),
        watched: &watched,
        agents: AgentFacts {
            config: None,
            versions: &BTreeMap::new(),
        },
    };
    run_watching(repo, store, state, agent, setup).await
}

/// A project's settings for [`run_project`].
#[derive(Debug, Clone, Copy)]
pub struct Project<'a> {
    /// The project's `harness.toml`.
    pub config: &'a Config,
    /// The roles' skills from `harness.toml`.
    pub skills: &'a Skills,
    /// What each agent's `--version` said, for the steps' manifests.
    pub agent_versions: &'a BTreeMap<AgentKind, String>,
}

/// Like [`run`], with the project's skills and settings: every role gets its
/// skills, and every step's manifest names the agent, model and versions.
pub async fn run_project<A: AgentRunner>(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
    agent: &A,
    project: Project<'_>,
) -> Result<StopReason, RunError> {
    let watched = Watched::for_repo(repo)?;
    let setup = RunSetup {
        skills: project.skills,
        watched: &watched,
        agents: AgentFacts {
            config: Some(project.config),
            versions: project.agent_versions,
        },
    };
    run_watching(repo, store, state, agent, setup).await
}

/// What every role of a run gets besides the agent.
#[derive(Debug, Clone, Copy)]
pub struct RunSetup<'a> {
    /// The roles' skills from `harness.toml`.
    pub skills: &'a Skills,
    /// The files the tripwire watches (see [`crate::task::tripwire`]).
    pub watched: &'a Watched,
    /// The settings and versions the steps' manifests record.
    pub agents: AgentFacts<'a>,
}

/// Like [`run_project`], with the files the tripwire watches given
/// (tests watch a temporary home folder instead of the real one).
pub async fn run_watching<A: AgentRunner>(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
    agent: &A,
    setup: RunSetup<'_>,
) -> Result<StopReason, RunError> {
    repo.ensure_harness_ignores()?;
    for _ in 0..MAX_STEPS_PER_RUN {
        let role = match state.stage {
            Stage::Done => return Ok(StopReason::Done),
            Stage::WaitingForHuman(reason) => return Ok(StopReason::WaitingForHuman(reason)),
            Stage::Working(role) => role,
        };

        // Before looking at git status: a block an unwatched run of Claude
        // Code left in `info/exclude` would hide files from it.
        agent_stubs::clear_leftovers(repo)?;
        // A limit stopped this role's last run in this round: its unfinished
        // changes are still in the folder, exactly as recorded, and the agent
        // continues from them. Any other change stops the run.
        let mut resumed = changes::resumable(RoleAt::new(repo, store, state, role))?;
        if resumed.is_none() {
            let dirty = repo.changed_files()?;
            if !dirty.is_empty() {
                return Ok(StopReason::DirtyWorkingTree(dirty));
            }
        }

        let mut problem = String::new();
        let mut accepted = false;
        // Failure logs the harness itself wrote during this role: they are not
        // the agent's changes.
        let mut failure_logs: Vec<PathBuf> = Vec::new();
        for _ in 0..ATTEMPTS_PER_ROLE {
            let before = Before::take(repo, setup.watched)?;
            let (job, manifest) = prepare_job(repo, store, state, role, setup, resumed.as_deref())?;
            let outcome = agent.run(&job).await;
            if let Some(stop) = before.check(repo, setup.watched, role)? {
                return Ok(stop);
            }
            if let Some(stop) =
                protect::put_back_protected(repo, store, state, role, &mut failure_logs)?
            {
                return Ok(stop);
            }
            match &outcome.end {
                RunEnd::Succeeded => {}
                RunEnd::UsageLimit | RunEnd::TimedOut(_) => {
                    failure_logs.push(store.save_failure_log(state.round, role, &outcome.log)?);
                    let at = RoleAt::new(repo, store, state, role);
                    return stop_at_limit(at, &outcome.end, &failure_logs);
                }
                RunEnd::Failed(message) => {
                    let log = store.save_failure_log(state.round, role, &outcome.log)?;
                    problem = format!(
                        "the agent failed: {} (full log: {})",
                        text::safe_line(message, 500),
                        log.display()
                    );
                    failure_logs.push(log);
                    let at = RoleAt::new(repo, store, state, role);
                    match after_failure(at, resumed.is_some(), &mut failure_logs, &mut problem)? {
                        AfterFailure::Stop(stop) => return Ok(stop),
                        AfterFailure::Retry(kept) => {
                            resumed = kept;
                            continue;
                        }
                    }
                }
            }
            if let Some(stop) = check_changes(repo, role, &failure_logs)? {
                return Ok(stop);
            }
            match accept_inbox(store, state)? {
                Acceptance::Accepted { handoff, step_dir } => {
                    store.save_log(&step_dir, &outcome.log)?;
                    store.save_manifest(&step_dir, &manifest)?;
                    // The unfinished work is now part of this step.
                    changes::remove_record(store)?;
                    repo.commit_all(&commit_message(&handoff))?;
                    accepted = true;
                    break;
                }
                Acceptance::Refused(why) => {
                    let log = store.save_failure_log(state.round, role, &outcome.log)?;
                    problem = format!("{why} (full log: {})", log.display());
                    failure_logs.push(log);
                    let at = RoleAt::new(repo, store, state, role);
                    match after_failure(at, resumed.is_some(), &mut failure_logs, &mut problem)? {
                        AfterFailure::Stop(stop) => return Ok(stop),
                        AfterFailure::Retry(kept) => resumed = kept,
                    }
                }
            }
        }
        if !accepted {
            commit_failure_logs(repo, &failure_logs, state, role, "failed")?;
            return Ok(StopReason::RoleFailed { role, problem });
        }
    }
    Ok(StopReason::StepLimitReached)
}

/// Commits only the failure logs (nothing an agent wrote), so the next run
/// starts with a clean project.
pub(super) fn commit_failure_logs(
    repo: &Repo,
    logs: &[PathBuf],
    state: &TaskState,
    role: Role,
    what: &str,
) -> Result<(), GitError> {
    let message = format!("{} round {}: {} {what}", state.task_id, state.round, role);
    let paths: Vec<&Path> = logs.iter().map(PathBuf::as_path).collect();
    repo.commit_paths(&paths, &message)?;
    Ok(())
}

/// Was the agent's handoff taken into the task?
enum Acceptance {
    /// Yes: the saved handoff and its step folder.
    Accepted { handoff: Handoff, step_dir: PathBuf },
    /// No: why, in one line for the next attempt's problem text.
    Refused(String),
}

/// Reads the agent's handoff and saves it as the next step. `Err` is only a
/// real harness error (for example the disk is full).
fn accept_inbox(store: &TaskStore, state: &mut TaskState) -> Result<Acceptance, StoreError> {
    let (handoff, notes) = match store.read_inbox() {
        Ok(found) => found,
        Err(e) => return Ok(Acceptance::Refused(format!("no valid handoff: {e}"))),
    };
    match store.record(state, &handoff, &notes) {
        Ok(step_dir) => Ok(Acceptance::Accepted { handoff, step_dir }),
        Err(StoreError::Refused(e)) => Ok(Acceptance::Refused(format!("handoff refused: {e}"))),
        Err(e) => Err(e),
    }
}

/// `task-001 round 2: tester (rejected) - Parser fails on empty input`
fn commit_message(handoff: &Handoff) -> String {
    const MAX_SUMMARY: usize = 60;
    let verdict = match handoff.verdict {
        Verdict::Approved => "approved",
        Verdict::Rejected => "rejected",
        Verdict::NeedsHuman => "needs human",
    };
    let first_line = handoff.summary.lines().next().unwrap_or("").trim();
    let mut summary: String = first_line.chars().take(MAX_SUMMARY).collect();
    if first_line.chars().count() > MAX_SUMMARY {
        summary.push('…');
    }
    format!(
        "{} round {}: {} ({verdict}) - {summary}",
        handoff.task_id, handoff.round, handoff.role,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::task::scenario::Workflow;

    #[test]
    fn only_the_most_exact_oversized_path_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let write = |path: &str, bytes: usize| {
            let path = dir.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, vec![b'x'; bytes]).unwrap();
        };
        write("src/main.rs", 10);
        write("app/node_modules/a/index.js", 60);
        write("app/node_modules/b/index.js", 60);
        write("app/package.json", 10);
        write("data.bin", 200);
        let files: Vec<String> = [
            "src/main.rs",
            "app/node_modules/a/index.js",
            "app/node_modules/b/index.js",
            "app/package.json",
            "data.bin",
            "gone.txt", // deleted: no size
        ]
        .map(String::from)
        .to_vec();

        assert_eq!(
            oversized_changes(dir.path(), &files, 100),
            vec![
                ("app/node_modules".to_string(), 120),
                ("data.bin".to_string(), 200),
            ]
        );
        // A folder git lists as a whole (a nested repository) is counted too.
        assert_eq!(
            oversized_changes(dir.path(), &["app/".to_string()], 100),
            vec![("app".to_string(), 130)]
        );
        // Many small parts: the change as a whole is too big.
        assert_eq!(
            oversized_changes(dir.path(), &files[..4], 150),
            Vec::<(String, u64)>::new()
        );
        assert_eq!(
            oversized_changes(dir.path(), &files[..4], 135),
            vec![(".".to_string(), 140)]
        );
        assert_eq!(
            oversized_changes(dir.path(), &files, 1000),
            Vec::<(String, u64)>::new()
        );
    }

    #[test]
    fn the_same_text_does_not_make_a_second_unfinished_task() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        create_task(
            &repo,
            "task-001",
            "Fix the demo:\n  two defects.",
            Workflow::Task,
            5,
        )
        .unwrap();

        // A copy that differs only in spaces and line breaks is the same task.
        let error = create_task(
            &repo,
            "task-002",
            "Fix the demo: two defects.\n",
            Workflow::Task,
            5,
        )
        .unwrap_err();
        assert!(
            matches!(&error, RunError::SameTask { task, .. } if task == "task-001"),
            "{error:?}"
        );
        assert!(
            error.to_string().contains("task-001 already has this text"),
            "{error}"
        );
        assert!(!repo.runs_dir().join("task-002").exists());

        // Another text is a new task; on purpose, the same text too.
        create_task(
            &repo,
            "task-002",
            "Fix the demo: one defect.",
            Workflow::Task,
            5,
        )
        .unwrap();
        create_task_anyway(
            &repo,
            "task-003",
            "Fix the demo: two defects.",
            Workflow::Task,
            5,
        )
        .unwrap();

        // A finished task may be given again.
        let runs = repo.runs_dir();
        for id in ["task-001", "task-003"] {
            std::fs::write(
                runs.join(id).join("state.json"),
                format!(r#"{{"task_id":"{id}","round":1,"max_rounds":5,"stage":"done"}}"#),
            )
            .unwrap();
        }
        assert_eq!(
            same_task(&runs, "Fix the demo: two defects.").unwrap(),
            None
        );
        create_task(
            &repo,
            "task-004",
            "Fix the demo: two defects.",
            Workflow::Task,
            5,
        )
        .unwrap();
    }
}
