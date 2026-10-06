//! The loop that runs roles one after another until the task has to stop.
//!
//! Around every role the harness uses git:
//!
//! 1. Before: the project must have no uncommitted changes, so everything that
//!    changes afterwards was done by this role.
//! 2. After: the changed files are checked against the role's permissions
//!    and against [`MAX_CHANGE_BYTES`], so build output never lands in git.
//! 3. If the role's work is accepted, everything is committed:
//!    `task-001 round 2: tester (rejected) - ...`. A failed attempt is thrown away.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::git::{GitError, Repo};
use crate::skills::Skills;
use crate::task::agent::{AgentRunner, RoleJob};
use crate::task::handoff::{Handoff, NextStep, Role, Verdict};
use crate::task::permissions;
use crate::task::prompt;
use crate::task::store::{self, StoreError, TaskStore};
use crate::task::{Stage, TaskState, WaitReason};
use crate::text;

/// How many times one role may try before the harness gives up and asks Lisa.
pub const ATTEMPTS_PER_ROLE: u32 = 2;

/// A role's change bigger than this (one file, or the new files of one folder)
/// is not committed. It is almost always build output or downloaded packages
/// (`target/`, `node_modules/`, `bin/`) that belong in `.gitignore`, whatever
/// the project is written in.
pub const MAX_CHANGE_BYTES: u64 = 50 * 1024 * 1024;

/// Safety net: never run more than this many roles in one call to `run`.
pub const MAX_STEPS_PER_RUN: u32 = 50;

/// Why `run` returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    Done,
    WaitingForHuman(WaitReason),
    UsageLimitReached(Role),
    /// The role did not produce an acceptable handoff after every attempt.
    RoleFailed {
        role: Role,
        problem: String,
    },
    StepLimitReached,
    /// The project had uncommitted changes before a role started. Lisa commits
    /// or removes them, then runs again.
    DirtyWorkingTree(Vec<String>),
    /// The role changed files it may not touch. The changes are left in place,
    /// uncommitted, so Lisa can look at them.
    ForbiddenChanges {
        role: Role,
        files: Vec<String>,
    },
    /// The role's change is too big to commit (see [`MAX_CHANGE_BYTES`]):
    /// each entry is a file or folder and its size in bytes, `.` meaning the
    /// change as a whole. Everything is left in place, uncommitted, so Lisa
    /// can add it to `.gitignore` or delete it.
    TooLarge {
        role: Role,
        files: Vec<(String, u64)>,
    },
    /// The agent made a git commit itself, which agents must never do.
    AgentCommitted(Role),
    /// The agent changed the repository's settings (`.git/config`), where a
    /// setting can make git run any command. The old settings are put back;
    /// the role's other changes are left uncommitted for Lisa to look at.
    GitConfigChanged(Role),
}

/// A real problem of the harness itself, not a mistake of an agent.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Git(#[from] GitError),
    /// An unfinished task already has this text: a second one is not created.
    #[error(
        "{task} already has this text and is not done ({stage}); continue it or change the text"
    )]
    SameTask { task: String, stage: String },
}

/// Creates a new task in `<project>/.harness/runs/` and commits it. If an
/// unfinished task already has the same text (a message sent twice), nothing
/// is created and the error names that task.
pub fn create_task(
    repo: &Repo,
    task_id: &str,
    description: &str,
    max_rounds: u32,
) -> Result<(TaskStore, TaskState), RunError> {
    if let Some((task, stage)) = same_task(&repo.runs_dir(), description)? {
        return Err(RunError::SameTask {
            task,
            stage: crate::retro::stage_text(stage),
        });
    }
    create_task_anyway(repo, task_id, description, max_rounds)
}

/// Like [`create_task`], but also when an unfinished task has the same text.
pub fn create_task_anyway(
    repo: &Repo,
    task_id: &str,
    description: &str,
    max_rounds: u32,
) -> Result<(TaskStore, TaskState), RunError> {
    repo.ensure_harness_ignores()?;
    let runs = repo.runs_dir();
    std::fs::create_dir_all(&runs).map_err(|source| StoreError::Io {
        path: runs.clone(),
        source,
    })?;
    let (store, state) = TaskStore::create(&runs, task_id, description, max_rounds)?;
    repo.commit_paths(&[store.dir()], &format!("{task_id}: new task"))?;
    Ok((store, state))
}

/// The unfinished task whose text is `description`, and its stage. Spaces
/// and line breaks do not count: a pasted copy of the same text matches.
pub fn same_task(runs: &Path, description: &str) -> Result<Option<(String, Stage)>, StoreError> {
    if !runs.is_dir() {
        return Ok(None);
    }
    let wanted = words(description);
    for id in store::task_ids(runs)? {
        // A task that cannot be read is no reason to refuse a new one.
        let Ok((task, state)) = TaskStore::open(runs, &id) else {
            continue;
        };
        let Ok(text) = task.description() else {
            continue;
        };
        if state.stage != Stage::Done && words(&text) == wanted {
            return Ok(Some((id, state.stage)));
        }
    }
    Ok(None)
}

/// The words of `text`, so that only spaces and line breaks may differ.
pub fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

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
            let head = repo.head()?;
            let config = repo.local_config()?;
            let job = prepare_job(repo.root(), store, state, role, skills)?;
            let outcome = agent.run(&job).await;

            // First of all, before git runs again with the agent's settings.
            if repo.local_config()? != config {
                repo.restore_local_config(&config)?;
                return Ok(StopReason::GitConfigChanged(role));
            }
            if repo.head()? != head {
                return Ok(StopReason::AgentCommitted(role));
            }
            if outcome.usage_limit_reached {
                repo.discard_changes()?;
                failure_logs.push(store.save_failure_log(state.round, role, &outcome.log)?);
                commit_failure_logs(repo, &failure_logs, state, role, "paused by usage limit")?;
                return Ok(StopReason::UsageLimitReached(role));
            }
            if !outcome.success {
                repo.discard_changes()?;
                let log = store.save_failure_log(state.round, role, &outcome.log)?;
                problem = format!(
                    "the agent failed: {} (full log: {})",
                    text::safe_line(&outcome.message, 500),
                    log.display()
                );
                failure_logs.push(log);
                continue;
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
                Ok((handoff, step_dir)) => {
                    store.save_log(&step_dir, &outcome.log)?;
                    repo.commit_all(&commit_message(&handoff))?;
                    accepted = true;
                    break;
                }
                Err(why) => {
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

/// The changed paths (as `git status` lists them, relative to `root`) that
/// are bigger than `limit`, with their sizes. A folder counts the files under
/// it; only the most exact path is named: `app/node_modules`, not `app` too.
/// `.` stands for the change as a whole, when no single part is too big.
pub fn oversized_changes(root: &Path, files: &[String], limit: u64) -> Vec<(String, u64)> {
    // The size of every changed path and of every folder above it; "" is the
    // whole change.
    let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
    for file in files {
        let size = size_of(&root.join(file));
        if size == 0 {
            continue;
        }
        let path = file.trim_end_matches('/');
        *sizes.entry(String::new()).or_default() += size;
        for (i, c) in path.char_indices() {
            if c == '/' {
                *sizes.entry(path[..i].to_string()).or_default() += size;
            }
        }
        *sizes.entry(path.to_string()).or_default() += size;
    }
    let over: Vec<&String> = sizes
        .iter()
        .filter(|(_, size)| **size > limit)
        .map(|(path, _)| path)
        .collect();
    let inside = |inner: &str, outer: &str| {
        outer.is_empty() && !inner.is_empty() || inner.starts_with(&format!("{outer}/"))
    };
    over.iter()
        .filter(|outer| !over.iter().any(|inner| inside(inner, outer)))
        .map(|path| {
            let name = if path.is_empty() { "." } else { path.as_str() };
            (name.to_string(), sizes[*path])
        })
        .collect()
}

/// Bytes in a file, or in all files under a folder. Links are not followed.
fn size_of(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::read_dir(path)
            .map(|entries| entries.flatten().map(|e| size_of(&e.path())).sum())
            .unwrap_or(0),
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}

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
    project_dir: &Path,
    store: &TaskStore,
    state: &TaskState,
    role: Role,
    skills: &Skills,
) -> Result<RoleJob, StoreError> {
    let output_dir = store.prepare_inbox()?;
    let history = store.history()?;
    let prompt = prompt::build(
        role,
        &store.description()?,
        state,
        history.last(),
        &output_dir,
        &skills.for_role(role),
    );
    Ok(RoleJob {
        task_id: state.task_id.clone(),
        round: state.round,
        role,
        project_dir: project_dir.to_path_buf(),
        prompt,
        output_dir,
    })
}

/// Outer `Result`: a real harness error. Inner `Result`: was the agent's work
/// accepted? If yes, it holds the saved handoff and its step folder.
fn accept_inbox(
    store: &TaskStore,
    state: &mut TaskState,
) -> Result<Result<(Handoff, PathBuf), String>, StoreError> {
    let (handoff, notes) = match store.read_inbox() {
        Ok(found) => found,
        Err(e) => return Ok(Err(format!("no valid handoff: {e}"))),
    };
    match store.record(state, &handoff, &notes) {
        Ok(step_dir) => Ok(Ok((handoff, step_dir))),
        Err(StoreError::Refused(e)) => Ok(Err(format!("handoff refused: {e}"))),
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
        assert!(oversized_changes(dir.path(), &files[..4], 150).is_empty());
        assert_eq!(
            oversized_changes(dir.path(), &files[..4], 135),
            vec![(".".to_string(), 140)]
        );
        assert!(oversized_changes(dir.path(), &files, 1000).is_empty());
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
