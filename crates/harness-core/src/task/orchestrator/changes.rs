//! What happens to a role's changes after an attempt that was not accepted:
//! checked, kept in the folder for the next run, or thrown away.
//!
//! When a limit (time or usage) or the user's «Stop now» stops a role, its
//! changes stay in the project folder, uncommitted, and `unfinished.json` in
//! the task's folder records which files they are and a hash of each. The
//! next run of the same role in the same round accepts exactly that dirty
//! folder and tells the agent to continue; anything else (the user edited a file,
//! also under `.harness/` without committing it, or committed a change to the
//! project) stops the run as before. No commit is made for the unfinished
//! work: every step stays one commit, which the facts and the weakened-checks
//! report read.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::{is_harness, FileFingerprint, GitError, Repo};
use crate::task::agent::RunEnd;
use crate::task::handoff::Role;
use crate::task::permissions;
use crate::task::store::{StoreError, TaskStore};
use crate::task::{TaskState, WaitReason};

use super::{oversized_changes, RunError, StopReason, MAX_CHANGE_BYTES};

/// The record of unfinished work, in the task's folder.
const UNFINISHED_FILE: &str = "unfinished.json";

/// Unfinished work a limit left in the folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Unfinished {
    /// The round it belongs to.
    round: u32,
    /// The role that made it.
    role: Role,
    /// The changed files and their hashes when the role stopped.
    files: Vec<FileFingerprint>,
}

/// The role being run, and where: what every function here needs.
#[derive(Debug, Clone, Copy)]
pub(super) struct RoleAt<'a> {
    /// The project's repository.
    pub repo: &'a Repo,
    /// The task's folder.
    pub store: &'a TaskStore,
    /// The task's state (its round).
    pub state: &'a TaskState,
    /// The role.
    pub role: Role,
}

impl<'a> RoleAt<'a> {
    /// The role `role` of the task in `store`, in `repo`.
    pub(super) fn new(
        repo: &'a Repo,
        store: &'a TaskStore,
        state: &'a TaskState,
        role: Role,
    ) -> Self {
        Self {
            repo,
            store,
            state,
            role,
        }
    }
}

/// Where the record of unfinished work is.
fn unfinished_path(store: &TaskStore) -> PathBuf {
    store.dir().join(UNFINISHED_FILE)
}

/// The record's path relative to the project, with `/` (also on Windows).
fn record_in_repo(at: RoleAt) -> String {
    let path = unfinished_path(at.store);
    let relative = path.strip_prefix(at.repo.root()).unwrap_or(&path);
    relative.to_string_lossy().replace('\\', "/")
}

/// Records the folder's changes as the role's unfinished work and adds the
/// record to `paths` (the harness's files to commit). With no changes, an
/// old record is deleted instead. Returns the changed files.
fn keep_unfinished(at: RoleAt, paths: &mut Vec<PathBuf>) -> Result<Vec<String>, RunError> {
    let files = at.repo.changes_fingerprint()?;
    let names: Vec<String> = files.iter().map(|(file, _)| file.clone()).collect();
    let path = unfinished_path(at.store);
    if files.is_empty() {
        remove_record(at.store)?;
    } else {
        let record = Unfinished {
            round: at.state.round,
            role: at.role,
            files,
        };
        let json = serde_json::to_string_pretty(&record).map_err(|source| StoreError::Json {
            path: path.clone(),
            source,
        })?;
        fs::write(&path, json + "\n").map_err(|source| StoreError::Io {
            path: path.clone(),
            source,
        })?;
    }
    // A deleted record is committed too, but only if git knows the file.
    let exists = path.exists() || at.repo.is_tracked(&path);
    if exists && !paths.contains(&path) {
        paths.push(path);
    }
    Ok(names)
}

/// The changed files left by a limit for this role in this round, when the
/// folder still holds exactly them; `None` otherwise (also without a record).
pub(super) fn resumable(at: RoleAt) -> Result<Option<Vec<String>>, GitError> {
    let Some(record) = read_record(&unfinished_path(at.store)) else {
        return Ok(None);
    };
    if record.round != at.state.round || record.role != at.role {
        return Ok(None);
    }
    // The record is committed when the role stops. A later commit may only
    // touch `.harness/` (the user raised the time limit in `harness.toml`); one
    // that changes the project means someone worked on the branch since.
    let Some(committed_since) = at.repo.changed_since_last_change(&record_in_repo(at)) else {
        return Ok(None);
    };
    if !committed_since.iter().all(|path| is_harness(path)) {
        return Ok(None);
    }
    // The fingerprint leaves out `.harness/`; nothing there may be changed.
    if at.repo.changed_files()?.iter().any(|path| is_harness(path)) {
        return Ok(None);
    }
    if at.repo.changes_fingerprint()? != record.files {
        return Ok(None);
    }
    Ok(Some(
        record.files.into_iter().map(|(path, _)| path).collect(),
    ))
}

/// The files of the unfinished work recorded in the task's folder, of any
/// round or role; empty without a record. Cleaning up must leave them alone,
/// or the work could no longer be resumed.
pub(super) fn recorded_files(store: &TaskStore) -> Vec<String> {
    read_record(&unfinished_path(store))
        .map(|record| record.files.into_iter().map(|(path, _)| path).collect())
        .unwrap_or_default()
}

/// The recorded unfinished work that is still uncommitted in the folder;
/// empty when there is none. Finishing the task would leave it stranded.
pub fn unfinished_left(repo: &Repo, store: &TaskStore) -> Result<Vec<String>, GitError> {
    let recorded = recorded_files(store);
    if recorded.is_empty() {
        return Ok(Vec::new());
    }
    let changed = repo.changed_files()?;
    Ok(recorded
        .into_iter()
        .filter(|file| changed.contains(file))
        .collect())
}

/// The record, or `None` when it is missing or unreadable (then nothing is
/// resumed and a dirty folder stops the run, as without this feature).
fn read_record(path: &Path) -> Option<Unfinished> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// The changes the agent made that may not be committed or kept: files its
/// role may not touch, or files too large. They are left in place for the user.
pub(super) fn check_changes(
    repo: &Repo,
    role: Role,
    failure_logs: &[PathBuf],
) -> Result<Option<StopReason>, GitError> {
    let agent_changes: Vec<String> = repo
        .changed_files()?
        .into_iter()
        .filter(|path| !failure_logs.contains(&repo.root().join(path)))
        .collect();
    let forbidden = permissions::forbidden_changes(role, &agent_changes);
    if !forbidden.is_empty() {
        return Ok(Some(StopReason::ForbiddenChanges {
            role,
            files: forbidden,
        }));
    }
    let too_large = oversized_changes(repo.root(), &agent_changes, MAX_CHANGE_BYTES);
    if !too_large.is_empty() {
        return Ok(Some(StopReason::TooLarge {
            role,
            files: too_large,
        }));
    }
    Ok(None)
}

/// Stops the run after a limit (time or usage) or the user's «Stop now»: the
/// role's work stays in the folder for the next run of this role, after the
/// same checks as accepted work; only the harness's records are committed.
/// Running again at once would hit the same limit, and throwing the work away
/// would lose it.
pub(super) fn stop_at_limit(
    at: RoleAt,
    end: &RunEnd,
    failure_logs: &[PathBuf],
) -> Result<StopReason, RunError> {
    let mut paths = failure_logs.to_vec();
    let (what, limit) = match end {
        RunEnd::UsageLimit => (
            "paused by usage limit",
            StopReason::UsageLimitReached(at.role),
        ),
        // The caller then moves the task to waiting for the user.
        RunEnd::StoppedByUser => (
            "stopped by the user",
            StopReason::WaitingForHuman(WaitReason::StoppedByUser(at.role)),
        ),
        // Only the limits come here; a finished or failed run never does.
        RunEnd::TimedOut(_) | RunEnd::Succeeded | RunEnd::Failed(_) => {
            ("stopped at the time limit", StopReason::TimedOut(at.role))
        }
    };
    let mut stop = limit;
    if let Some(refused) = check_changes(at.repo, at.role, failure_logs)? {
        // Changes the checks refuse are not unfinished work: the user decides.
        remove_record(at.store)?;
        let path = unfinished_path(at.store);
        if at.repo.is_tracked(&path) {
            paths.push(path);
        }
        stop = refused;
    } else {
        keep_unfinished(at, &mut paths)?;
    }
    super::commit_failure_logs(at.repo, &paths, at.state, at.role, what)?;
    Ok(stop)
}

/// What to do after an attempt that failed or whose handoff was refused.
pub(super) enum AfterFailure {
    /// The checks stopped the run.
    Stop(StopReason),
    /// Try again. `Some` lists the resumed work that stays in the folder;
    /// `None` means the folder was cleaned.
    Retry(Option<Vec<String>>),
}

/// After a failed attempt: resumed work stays in the folder, recorded again
/// (after the checks), and `problem` says so; the next attempt continues
/// from it. Without resumed work the attempt is thrown away as before.
pub(super) fn after_failure(
    at: RoleAt,
    resumed: bool,
    failure_logs: &mut Vec<PathBuf>,
    problem: &mut String,
) -> Result<AfterFailure, RunError> {
    if !resumed {
        at.repo.discard_changes()?;
        return Ok(AfterFailure::Retry(None));
    }
    if let Some(stop) = check_changes(at.repo, at.role, failure_logs)? {
        return Ok(AfterFailure::Stop(stop));
    }
    let files = keep_unfinished(at, failure_logs)?;
    if files.is_empty() {
        return Ok(AfterFailure::Retry(None));
    }
    problem.push_str("; its unfinished work stays in the folder for the next run");
    Ok(AfterFailure::Retry(Some(files)))
}

/// Deletes the record of unfinished work, if there is one.
pub(super) fn remove_record(store: &TaskStore) -> Result<(), StoreError> {
    let path = unfinished_path(store);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(StoreError::Io { path, source }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::scenario::Workflow;

    /// A repository with one commit and a task in it.
    fn setup() -> (tempfile::TempDir, Repo, TaskStore, TaskState) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        fs::create_dir_all(repo.runs_dir()).unwrap();
        let (store, state) =
            TaskStore::create(&repo.runs_dir(), "task-001", "Do it.", Workflow::Task, 5).unwrap();
        repo.commit_all("first").unwrap();
        (dir, repo, store, state)
    }

    /// Writes the record for the folder as it is now and commits it.
    fn keep(at: RoleAt) {
        let mut paths = Vec::new();
        keep_unfinished(at, &mut paths).unwrap();
        let paths: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        at.repo.commit_paths(&paths, "record").unwrap();
    }

    #[test]
    fn the_same_role_resumes_exactly_the_recorded_work() {
        let (dir, repo, store, state) = setup();
        fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        let at = RoleAt {
            repo: &repo,
            store: &store,
            state: &state,
            role: Role::Developer,
        };
        keep(at);

        assert_eq!(resumable(at).unwrap(), Some(vec!["a.txt".to_string()]));
        let other_role = RoleAt {
            role: Role::Tester,
            ..at
        };
        assert_eq!(resumable(other_role).unwrap(), None);
        let mut next_round = state.clone();
        next_round.round += 1;
        let later = RoleAt {
            state: &next_round,
            ..at
        };
        assert_eq!(resumable(later).unwrap(), None);
    }

    #[test]
    fn any_other_change_is_not_resumed() {
        let (dir, repo, store, state) = setup();
        fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        let at = RoleAt {
            repo: &repo,
            store: &store,
            state: &state,
            role: Role::Developer,
        };
        keep(at);

        // The user's own file under `.harness/`, outside the fingerprint.
        fs::write(dir.path().join(".harness/note.md"), "mine\n").unwrap();
        assert_eq!(resumable(at).unwrap(), None);
        fs::remove_file(dir.path().join(".harness/note.md")).unwrap();
        // A new file of theirs next to the agent's.
        fs::write(dir.path().join("b.txt"), "mine\n").unwrap();
        assert_eq!(resumable(at).unwrap(), None);
        fs::remove_file(dir.path().join("b.txt")).unwrap();
        // A changed file of the agent's.
        fs::write(dir.path().join("a.txt"), "three\n").unwrap();
        assert_eq!(resumable(at).unwrap(), None);
    }

    #[test]
    fn a_new_commit_on_the_branch_is_not_resumed() {
        let (dir, repo, store, state) = setup();
        fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        let at = RoleAt {
            repo: &repo,
            store: &store,
            state: &state,
            role: Role::Developer,
        };
        keep(at);
        fs::write(dir.path().join("c.txt"), "c\n").unwrap();
        repo.commit_paths(&[Path::new("c.txt")], "someone else")
            .unwrap();

        assert_eq!(resumable(at).unwrap(), None);
    }

    #[test]
    fn a_committed_settings_change_still_resumes() {
        let (dir, repo, store, state) = setup();
        fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        let at = RoleAt {
            repo: &repo,
            store: &store,
            state: &state,
            role: Role::Developer,
        };
        keep(at);
        fs::write(
            dir.path().join(".harness/harness.toml"),
            "agent_timeout_minutes = 150\n",
        )
        .unwrap();
        repo.commit_paths(&[Path::new(".harness/harness.toml")], "more time")
            .unwrap();

        assert_eq!(resumable(at).unwrap(), Some(vec!["a.txt".to_string()]));
    }

    #[test]
    fn no_changes_delete_an_old_record() {
        let (dir, repo, store, state) = setup();
        fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        let at = RoleAt {
            repo: &repo,
            store: &store,
            state: &state,
            role: Role::Developer,
        };
        keep(at);
        repo.discard_changes().unwrap();

        let mut paths = Vec::new();
        let files = keep_unfinished(at, &mut paths).unwrap();

        assert_eq!(files, Vec::<String>::new());
        assert!(!unfinished_path(&store).exists());
        assert_eq!(
            paths,
            vec![unfinished_path(&store)],
            "the deletion is committed"
        );
    }
}
