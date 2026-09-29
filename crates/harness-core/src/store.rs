//! Saving tasks to disk, so the harness can stop and continue later.
//!
//! One task lives in its own folder:
//!
//! ```text
//! runs/task-001/
//!   task.md              the task description
//!   state.json           the current TaskState
//!   round-01/
//!     01-architect/      handoff.json + notes.md
//!     02-human/
//!     03-developer/
//!   round-02/
//!     01-developer/
//!   inbox/               scratch folder where an agent writes its result
//! ```
//!
//! Step folders are numbered because the same role can work twice in one round
//! (for example the tester asks Lisa for help and then continues).
//! Nothing is ever overwritten.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Serialize};

use crate::handoff::{Handoff, Role};
use crate::task::{TaskState, TransitionError};

const STATE_FILE: &str = "state.json";
const TASK_FILE: &str = "task.md";
const HANDOFF_FILE: &str = "handoff.json";
const NOTES_FILE: &str = "notes.md";
const INBOX_DIR: &str = "inbox";
const LOG_FILE: &str = "agent.log";
const FAILURES_DIR: &str = "failures";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("task id {0:?} may only contain lowercase letters, digits and '-'")]
    InvalidTaskId(String),
    #[error("task {0} already exists")]
    TaskExists(String),
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not valid JSON for this file: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("handoff refused: {0}")]
    Refused(#[from] TransitionError),
}

/// Access to one task's folder.
#[derive(Debug)]
pub struct TaskStore {
    dir: PathBuf,
}

impl TaskStore {
    /// Creates the folder for a new task and saves its first state.
    pub fn create(
        runs_dir: &Path,
        task_id: &str,
        description: &str,
        max_rounds: u32,
    ) -> Result<(Self, TaskState), StoreError> {
        check_task_id(task_id)?;
        let dir = runs_dir.join(task_id);
        if dir.exists() {
            return Err(StoreError::TaskExists(task_id.to_string()));
        }
        create_dir(&dir)?;

        let store = Self { dir };
        let state = TaskState::new(task_id, max_rounds);
        write_new(&store.dir.join(TASK_FILE), description)?;
        store.save_state(&state)?;
        Ok((store, state))
    }

    /// Opens an existing task and reads where it stopped.
    pub fn open(runs_dir: &Path, task_id: &str) -> Result<(Self, TaskState), StoreError> {
        check_task_id(task_id)?;
        let store = Self {
            dir: runs_dir.join(task_id),
        };
        let state = read_json(&store.dir.join(STATE_FILE))?;
        Ok((store, state))
    }

    /// Checks a handoff, saves it with its notes, and moves the task forward.
    ///
    /// If anything fails, `state` is left unchanged.
    pub fn record(
        &self,
        state: &mut TaskState,
        handoff: &Handoff,
        notes: &str,
    ) -> Result<PathBuf, StoreError> {
        // Try the move on a copy first, so a refused handoff changes nothing.
        let mut next = state.clone();
        next.apply(handoff)?;

        // The handoff belongs to the round it was written in (before any `round += 1`).
        let step_dir = self.new_step_dir(state.round, handoff.role)?;
        write_json_new(&step_dir.join(HANDOFF_FILE), handoff)?;
        write_new(&step_dir.join(NOTES_FILE), notes)?;
        self.save_state(&next)?;

        *state = next;
        Ok(step_dir)
    }

    /// Saves what the agent printed next to its handoff, as `agent.log`.
    pub fn save_log(&self, step_dir: &Path, log: &str) -> Result<(), StoreError> {
        write_new(&step_dir.join(LOG_FILE), log)
    }

    /// Saves the log of a failed attempt as `failures/round-01-architect-1.log`,
    /// so Lisa can see what went wrong. Returns the file's path.
    pub fn save_failure_log(
        &self,
        round: u32,
        role: Role,
        log: &str,
    ) -> Result<PathBuf, StoreError> {
        let dir = self.dir.join(FAILURES_DIR);
        if !dir.exists() {
            create_dir(&dir)?;
        }
        let prefix = format!("round-{round:02}-{}-", role_name(role));
        let mut attempt = 1;
        loop {
            let path = dir.join(format!("{prefix}{attempt}.log"));
            if !path.exists() {
                write_new(&path, log)?;
                return Ok(path);
            }
            attempt += 1;
        }
    }

    /// The task description written by Lisa (`task.md`).
    pub fn description(&self) -> Result<String, StoreError> {
        let path = self.dir.join(TASK_FILE);
        fs::read_to_string(&path).map_err(|e| io_error(&path, e))
    }

    /// Empties the inbox and returns its path. An agent writes its
    /// `handoff.json` and `notes.md` there; nothing in it is kept as history.
    pub fn prepare_inbox(&self) -> Result<PathBuf, StoreError> {
        let inbox = self.dir.join(INBOX_DIR);
        if inbox.exists() {
            fs::remove_dir_all(&inbox).map_err(|e| io_error(&inbox, e))?;
        }
        create_dir(&inbox)?;
        Ok(inbox)
    }

    /// Reads what the agent left in the inbox. Missing notes are allowed, a missing
    /// or invalid handoff is not.
    pub fn read_inbox(&self) -> Result<(Handoff, String), StoreError> {
        let inbox = self.dir.join(INBOX_DIR);
        let handoff = read_json(&inbox.join(HANDOFF_FILE))?;
        let notes = fs::read_to_string(inbox.join(NOTES_FILE)).unwrap_or_default();
        Ok((handoff, notes))
    }

    /// All saved handoffs, in the order they happened.
    pub fn history(&self) -> Result<Vec<Handoff>, StoreError> {
        let mut handoffs = Vec::new();
        let rounds = sorted_subdirs(&self.dir)?.into_iter().filter(|dir| {
            dir.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("round-"))
        });
        for round_dir in rounds {
            for step_dir in sorted_subdirs(&round_dir)? {
                handoffs.push(read_json(&step_dir.join(HANDOFF_FILE))?);
            }
        }
        Ok(handoffs)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The failed attempts saved by `save_failure_log`, as (round, role), in order.
    /// Files with other names are skipped.
    pub fn failures(&self) -> Result<Vec<(u32, Role)>, StoreError> {
        let dir = self.dir.join(FAILURES_DIR);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| io_error(&dir, e))? {
            let entry = entry.map_err(|e| io_error(&dir, e))?;
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names
            .iter()
            .filter_map(|name| parse_failure(name))
            .collect())
    }

    /// Creates the next numbered step folder, e.g. `round-02/03-tester`.
    fn new_step_dir(&self, round: u32, role: Role) -> Result<PathBuf, StoreError> {
        let round_dir = self.dir.join(format!("round-{round:02}"));
        if !round_dir.exists() {
            create_dir(&round_dir)?;
        }
        let step = sorted_subdirs(&round_dir)?.len() + 1;
        let step_dir = round_dir.join(format!("{step:02}-{}", role_name(role)));
        create_dir(&step_dir)?;
        Ok(step_dir)
    }

    /// Saves the state safely: write a temporary file, then rename it.
    /// A rename is atomic, so a crash never leaves a half-written `state.json`.
    fn save_state(&self, state: &TaskState) -> Result<(), StoreError> {
        let path = self.dir.join(STATE_FILE);
        let tmp = self.dir.join("state.json.tmp");
        fs::write(&tmp, to_json(&tmp, state)?).map_err(|e| io_error(&tmp, e))?;
        fs::rename(&tmp, &path).map_err(|e| io_error(&path, e))
    }
}

/// The ids of all tasks in `runs_dir` (folders with a `state.json`), sorted.
pub fn task_ids(runs_dir: &Path) -> Result<Vec<String>, StoreError> {
    if !runs_dir.exists() {
        return Ok(Vec::new());
    }
    Ok(sorted_subdirs(runs_dir)?
        .into_iter()
        .filter(|dir| dir.join(STATE_FILE).exists())
        .filter_map(|dir| Some(dir.file_name()?.to_string_lossy().into_owned()))
        .filter(|id| check_task_id(id).is_ok())
        .collect())
}

/// `round-02-tester-1.log` -> (2, Tester).
fn parse_failure(name: &str) -> Option<(u32, Role)> {
    let rest = name.strip_prefix("round-")?.strip_suffix(".log")?;
    let (round, rest) = rest.split_once('-')?;
    let (role, attempt) = rest.rsplit_once('-')?;
    attempt.parse::<u32>().ok()?;
    let role = serde_json::from_value(serde_json::Value::String(role.to_string())).ok()?;
    Some((round.parse().ok()?, role))
}

/// Task ids become folder names, so we allow only safe characters.
/// This blocks tricks like `../../etc`.
fn check_task_id(task_id: &str) -> Result<(), StoreError> {
    let ok = !task_id.is_empty()
        && task_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(StoreError::InvalidTaskId(task_id.to_string()))
    }
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Architect => "architect",
        Role::Developer => "developer",
        Role::Tester => "tester",
        Role::Security => "security",
        Role::Human => "human",
    }
}

fn io_error(path: &Path, source: io::Error) -> StoreError {
    StoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn create_dir(path: &Path) -> Result<(), StoreError> {
    fs::create_dir(path).map_err(|e| io_error(path, e))
}

/// Writes a file that must not exist yet: history is never overwritten.
fn write_new(path: &Path, contents: &str) -> Result<(), StoreError> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| io_error(path, e))?;
    file.write_all(contents.as_bytes())
        .map_err(|e| io_error(path, e))
}

fn to_json<T: Serialize>(path: &Path, value: &T) -> Result<String, StoreError> {
    serde_json::to_string_pretty(value).map_err(|source| StoreError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn write_json_new<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    write_new(path, &to_json(path, value)?)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
    let text = fs::read_to_string(path).map_err(|e| io_error(path, e))?;
    serde_json::from_str(&text).map_err(|source| StoreError::Json {
        path: path.to_path_buf(),
        source,
    })
}

/// Sub-folders of `dir`, sorted by name. Numbered names sort in time order.
fn sorted_subdirs(dir: &Path) -> Result<Vec<PathBuf>, StoreError> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| io_error(dir, e))? {
        let path = entry.map_err(|e| io_error(dir, e))?.path();
        if path.is_dir() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::{Issue, NextStep, Severity, Verdict};
    use crate::task::{Stage, WaitReason, DEFAULT_MAX_ROUNDS};

    fn handoff(role: Role, round: u32, verdict: Verdict, next: NextStep) -> Handoff {
        let issues = if verdict == Verdict::Rejected {
            vec![Issue {
                severity: Severity::High,
                location: None,
                description: "something is wrong".into(),
            }]
        } else {
            vec![]
        };
        Handoff {
            schema_version: 1,
            task_id: "task-001".into(),
            round,
            role,
            verdict,
            next_role: next,
            summary: format!("{role:?} finished"),
            skills_used: vec![],
            files: vec![],
            issues,
        }
    }

    fn new_task(runs: &Path) -> (TaskStore, TaskState) {
        TaskStore::create(runs, "task-001", "Build a parser", DEFAULT_MAX_ROUNDS).unwrap()
    }

    #[test]
    fn create_writes_task_and_state_files() {
        let runs = tempfile::tempdir().unwrap();
        let (store, _) = new_task(runs.path());
        let description = fs::read_to_string(store.dir().join("task.md")).unwrap();
        assert_eq!(description, "Build a parser");
        assert!(store.dir().join("state.json").exists());
    }

    #[test]
    fn cannot_create_the_same_task_twice() {
        let runs = tempfile::tempdir().unwrap();
        new_task(runs.path());
        let err = TaskStore::create(runs.path(), "task-001", "again", 5).unwrap_err();
        assert!(matches!(err, StoreError::TaskExists(_)));
    }

    #[test]
    fn refuses_unsafe_task_ids() {
        let runs = tempfile::tempdir().unwrap();
        for bad in ["", "../escape", "Task 1", "a/b"] {
            let err = TaskStore::create(runs.path(), bad, "x", 5).unwrap_err();
            assert!(matches!(err, StoreError::InvalidTaskId(_)), "{bad:?}");
        }
    }

    #[test]
    fn record_saves_handoff_and_notes_in_a_step_folder() {
        let runs = tempfile::tempdir().unwrap();
        let (store, mut state) = new_task(runs.path());
        let h = handoff(
            Role::Architect,
            1,
            Verdict::Approved,
            NextStep::To(Role::Human),
        );

        let step_dir = store.record(&mut state, &h, "Design is ready.").unwrap();

        assert!(step_dir.ends_with("round-01/01-architect"));
        let saved: Handoff = read_json(&step_dir.join("handoff.json")).unwrap();
        assert_eq!(saved, h);
        assert_eq!(
            fs::read_to_string(step_dir.join("notes.md")).unwrap(),
            "Design is ready."
        );
    }

    #[test]
    fn failures_and_task_ids_are_listed() {
        let runs = tempfile::tempdir().unwrap();
        assert!(task_ids(&runs.path().join("missing")).unwrap().is_empty());
        let (store, _) = new_task(runs.path());
        TaskStore::create(runs.path(), "task-002", "x", 5).unwrap();
        fs::create_dir(runs.path().join("not-a-task")).unwrap();
        assert_eq!(task_ids(runs.path()).unwrap(), ["task-001", "task-002"]);

        assert!(store.failures().unwrap().is_empty());
        store.save_failure_log(1, Role::Architect, "boom").unwrap();
        store.save_failure_log(2, Role::Tester, "boom").unwrap();
        store.save_failure_log(1, Role::Architect, "again").unwrap();
        fs::write(store.dir().join("failures/notes.txt"), "").unwrap();
        assert_eq!(
            store.failures().unwrap(),
            [
                (1, Role::Architect),
                (1, Role::Architect),
                (2, Role::Tester)
            ]
        );
    }

    #[test]
    fn continues_after_a_restart() {
        let runs = tempfile::tempdir().unwrap();
        {
            let (store, mut state) = new_task(runs.path());
            let h = handoff(
                Role::Architect,
                1,
                Verdict::Approved,
                NextStep::To(Role::Human),
            );
            store.record(&mut state, &h, "").unwrap();
        } // the store is dropped here, as if the program stopped

        let (_, state) = TaskStore::open(runs.path(), "task-001").unwrap();
        assert_eq!(
            state.stage,
            Stage::WaitingForHuman(WaitReason::ApproveDesign)
        );
    }

    #[test]
    fn a_refused_handoff_changes_nothing_on_disk() {
        let runs = tempfile::tempdir().unwrap();
        let (store, mut state) = new_task(runs.path());
        let before = state.clone();
        // The developer may not work before the architect.
        let h = handoff(
            Role::Developer,
            1,
            Verdict::Approved,
            NextStep::To(Role::Tester),
        );

        let err = store.record(&mut state, &h, "").unwrap_err();

        assert!(matches!(err, StoreError::Refused(_)));
        assert_eq!(state, before);
        assert!(!store.dir().join("round-01").exists());
        let (_, on_disk) = TaskStore::open(runs.path(), "task-001").unwrap();
        assert_eq!(on_disk, before);
    }

    #[test]
    fn a_rejection_is_saved_in_the_round_it_happened() {
        let runs = tempfile::tempdir().unwrap();
        let (store, mut state) = new_task(runs.path());
        state.stage = Stage::Working(Role::Tester);
        let h = handoff(
            Role::Tester,
            1,
            Verdict::Rejected,
            NextStep::To(Role::Developer),
        );

        let step_dir = store.record(&mut state, &h, "").unwrap();

        assert!(step_dir.ends_with("round-01/01-tester"));
        assert_eq!(state.round, 2);
    }

    #[test]
    fn same_role_twice_in_a_round_gets_two_folders() {
        let runs = tempfile::tempdir().unwrap();
        let (store, mut state) = new_task(runs.path());
        state.stage = Stage::Working(Role::Tester);
        let steps = [
            handoff(
                Role::Tester,
                1,
                Verdict::NeedsHuman,
                NextStep::To(Role::Human),
            ),
            handoff(
                Role::Human,
                1,
                Verdict::Approved,
                NextStep::To(Role::Tester),
            ),
            handoff(
                Role::Tester,
                1,
                Verdict::Approved,
                NextStep::To(Role::Security),
            ),
        ];
        for h in &steps {
            store.record(&mut state, h, "").unwrap();
        }
        let round_dir = store.dir().join("round-01");
        assert!(round_dir.join("01-tester").exists());
        assert!(round_dir.join("02-human").exists());
        assert!(round_dir.join("03-tester").exists());
    }

    #[test]
    fn history_returns_every_handoff_in_order() {
        let runs = tempfile::tempdir().unwrap();
        let (store, mut state) = new_task(runs.path());
        let steps = [
            handoff(
                Role::Architect,
                1,
                Verdict::Approved,
                NextStep::To(Role::Human),
            ),
            handoff(
                Role::Human,
                1,
                Verdict::Approved,
                NextStep::To(Role::Developer),
            ),
            handoff(
                Role::Developer,
                1,
                Verdict::Approved,
                NextStep::To(Role::Tester),
            ),
            handoff(
                Role::Tester,
                1,
                Verdict::Rejected,
                NextStep::To(Role::Developer),
            ),
            handoff(
                Role::Developer,
                2,
                Verdict::Approved,
                NextStep::To(Role::Tester),
            ),
        ];
        for h in &steps {
            store.record(&mut state, h, "").unwrap();
        }
        assert_eq!(store.history().unwrap(), steps);
    }
}
