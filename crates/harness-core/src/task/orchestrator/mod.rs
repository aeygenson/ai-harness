//! The loop that runs roles one after another until the task has to stop.
//!
//! Around every role the harness uses git:
//!
//! 1. Before: the project must have no uncommitted changes, so everything that
//!    changes afterwards was done by this role.
//! 2. After: git's hooks and a few files outside the project are compared
//!    with before (see [`crate::task::tripwire`]); files no role may change
//!    (agent instructions and settings, the harness's own files) are put back
//!    at once; then the changed files are
//!    checked against the role's permissions and against [`MAX_CHANGE_BYTES`],
//!    so build output never lands in git.
//! 3. If the role's work is accepted, everything is committed:
//!    `task-001 round 2: tester (rejected) - ...`. A failed attempt is thrown away.

mod create;
mod guard;
mod human;
mod protect;
mod size;
mod stop;

pub use create::{create_task, create_task_anyway, same_task, words};
pub use human::record_human_decision;
pub use size::oversized_changes;
pub use stop::{RunError, StopReason};

use std::path::{Path, PathBuf};

use crate::git::{GitError, Repo};
use crate::skills::Skills;
use crate::task::agent::{AgentRunner, RoleJob, RunEnd};
use crate::task::facts;
use crate::task::handoff::{Handoff, Role, Verdict};
use crate::task::permissions;
use crate::task::prompt;
use crate::task::store::{StoreError, TaskStore};
use crate::task::tripwire::Watched;
use crate::task::{Stage, TaskState};
use crate::text;
use guard::Before;

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
    run_with_skills(repo, store, state, agent, &Skills::none()).await
}

/// Like [`run`], but every role also gets its skills from `harness.toml`.
pub async fn run_with_skills<A: AgentRunner>(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
    agent: &A,
    skills: &Skills,
) -> Result<StopReason, RunError> {
    let watched = Watched::for_repo(repo)?;
    let setup = RunSetup {
        skills,
        watched: &watched,
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
}

/// Like [`run_with_skills`], with the files the tripwire watches given
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

        let dirty = repo.changed_files()?;
        if !dirty.is_empty() {
            return Ok(StopReason::DirtyWorkingTree(dirty));
        }

        let mut problem = String::new();
        let mut accepted = false;
        // Failure logs the harness itself wrote during this role: they are not
        // the agent's changes.
        let mut failure_logs: Vec<PathBuf> = Vec::new();
        for _ in 0..ATTEMPTS_PER_ROLE {
            let before = Before::take(repo, setup.watched)?;
            let job = prepare_job(repo, store, state, role, setup.skills)?;
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
                RunEnd::UsageLimit => {
                    repo.discard_changes()?;
                    failure_logs.push(store.save_failure_log(state.round, role, &outcome.log)?);
                    commit_failure_logs(repo, &failure_logs, state, role, "paused by usage limit")?;
                    return Ok(StopReason::UsageLimitReached(role));
                }
                RunEnd::Failed(message) => {
                    repo.discard_changes()?;
                    let log = store.save_failure_log(state.round, role, &outcome.log)?;
                    problem = format!(
                        "the agent failed: {} (full log: {})",
                        text::safe_line(message, 500),
                        log.display()
                    );
                    failure_logs.push(log);
                    continue;
                }
            }
            let agent_changes: Vec<String> = repo
                .changed_files()?
                .into_iter()
                .filter(|path| !failure_logs.contains(&repo.root().join(path)))
                .collect();
            let forbidden = permissions::forbidden_changes(role, &agent_changes);
            if !forbidden.is_empty() {
                return Ok(StopReason::ForbiddenChanges {
                    role,
                    files: forbidden,
                });
            }
            let too_large = oversized_changes(repo.root(), &agent_changes, MAX_CHANGE_BYTES);
            if !too_large.is_empty() {
                return Ok(StopReason::TooLarge {
                    role,
                    files: too_large,
                });
            }
            match accept_inbox(store, state)? {
                Acceptance::Accepted { handoff, step_dir } => {
                    store.save_log(&step_dir, &outcome.log)?;
                    repo.commit_all(&commit_message(&handoff))?;
                    accepted = true;
                    break;
                }
                Acceptance::Refused(why) => {
                    repo.discard_changes()?;
                    let log = store.save_failure_log(state.round, role, &outcome.log)?;
                    problem = format!("{why} (full log: {})", log.display());
                    failure_logs.push(log);
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
fn commit_failure_logs(
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

fn prepare_job(
    repo: &Repo,
    store: &TaskStore,
    state: &TaskState,
    role: Role,
    skills: &Skills,
) -> Result<RoleJob, StoreError> {
    let output_dir = store.prepare_inbox()?;
    let steps = store.steps()?;
    let facts = facts::text(repo, &steps);
    let context = prompt::Context {
        description: &store.description()?,
        previous: steps.last().map(|step| &step.handoff),
        facts: &facts,
        skills: &skills.for_role(role),
    };
    let prompt = prompt::build(role, state, context, &output_dir);
    Ok(RoleJob {
        task_id: state.task_id.clone(),
        round: state.round,
        role,
        project_dir: repo.root().to_path_buf(),
        prompt,
        output_dir,
    })
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
        create_task(&repo, "task-001", "Fix the demo:\n  two defects.", 5).unwrap();

        // A copy that differs only in spaces and line breaks is the same task.
        let error = create_task(&repo, "task-002", "Fix the demo: two defects.\n", 5).unwrap_err();
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
        create_task(&repo, "task-002", "Fix the demo: one defect.", 5).unwrap();
        create_task_anyway(&repo, "task-003", "Fix the demo: two defects.", 5).unwrap();

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
        create_task(&repo, "task-004", "Fix the demo: two defects.", 5).unwrap();
    }
}
