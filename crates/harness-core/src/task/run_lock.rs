//! One harness at a time in a project folder: the run lock, and the stop file
//! through which another program asks the running harness to stop.
//!
//! Two programs working in one folder at once (the TUI and `harness run` in a
//! terminal, or two terminals) would switch branches and commit under each
//! other's feet. So the commands that change a task (`harness task new`,
//! `run`, `approve`, `reject`, and the work the TUI's Tasks tab starts) first
//! hold the lock `.git/harness/run.lock`; a second one is refused and told
//! which task the first is busy with (`run.task` next to it).
//!
//! The files live in git's own folder, not in `.harness/`: git never shows
//! them as changes and they stay when a branch is switched. The operating
//! system holds the lock (see `harness_platform::lock`), so a crashed run
//! never leaves it behind.
//!
//! A run that runs roles makes an empty stop file `stop-<task>` there.
//! `harness stop <task>` writes into it; the running harness reads it through
//! its [`StopRequest`] and stops as if the user had pressed the TUI's button.
//! The file carries the task in its name, so a request that arrives late
//! never stops the next task. Work that cannot stop halfway (a decision, a
//! merge) makes no stop file, and `harness stop` says so.
//!
//! Agents run as the same user and a role with a shell could still change
//! these files; the checks below keep that from hanging or misleading the
//! harness, not from being possible (see AGENTS.md, Security).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use harness_platform::lock::{self, Held};

use crate::git::{GitError, Repo};
use crate::task::stop_request::{self, StopRequest, StopWhen};
use crate::task::store::{check_task_id, StoreError};

/// The folder inside `.git` with the lock and its small files.
const LOCK_DIR: &str = "harness";

/// The file the running harness holds the lock on.
const LOCK_FILE: &str = "run.lock";

/// The task the running harness works on, for the message to a second one.
const TASK_FILE: &str = "run.task";

/// Why the lock could not be taken or a stop could not be asked.
#[derive(Debug, thiserror::Error)]
pub enum RunLockError {
    /// Another harness works in this project folder right now.
    #[error("{}", busy_text(.task.as_deref(), *.can_stop))]
    Busy {
        /// The task it works on, when it has said so.
        task: Option<String>,
        /// Whether `harness stop` can stop it (it runs roles).
        can_stop: bool,
    },
    /// No harness runs roles of this task in this project right now.
    #[error("no harness is running {0} in this project right now")]
    NotRunning(String),
    /// A harness works on the task, but on a step that cannot stop halfway.
    #[error(
        "the harness is saving a decision on {0}, or sending or merging it; \
         that cannot stop halfway, wait until it finishes"
    )]
    CannotStop(String),
    /// The task id has characters a task id cannot have.
    #[error(transparent)]
    BadTaskId(#[from] StoreError),
    /// Git could not say where its folder is.
    #[error(transparent)]
    Git(#[from] GitError),
    /// A lock file could not be made, locked, read or written.
    #[error("cannot use the run lock {path}: {source}")]
    Io {
        /// The file or folder.
        path: PathBuf,
        /// The error the operating system gave.
        #[source]
        source: io::Error,
    },
}

/// The message for [`RunLockError::Busy`], with the command that helps.
fn busy_text(task: Option<&str>, can_stop: bool) -> String {
    match (task, can_stop) {
        (Some(task), true) => format!(
            "another harness is running {task} in this project; wait until it \
             finishes, or stop it with: harness stop {task}"
        ),
        (Some(task), false) => {
            format!("another harness is working on {task} in this project; wait until it finishes")
        }
        (None, _) => "another harness is working in this project; wait until it finishes".into(),
    }
}

/// The held lock of this project folder. Dropping it lets the next harness in.
#[derive(Debug)]
pub struct RunLock {
    dir: PathBuf,
    /// The task the work is on; `None` while a new task has no id yet.
    task: Option<String>,
    // Declared last, so it is dropped after `Drop::drop` has removed the
    // small files: nobody can take the lock and find our old ones.
    _held: Held,
}

impl RunLock {
    /// Takes the lock, or says who holds it. Then name the task with
    /// [`RunLock::set_task`]; a new task gets its id only after this.
    ///
    /// # Errors
    ///
    /// [`RunLockError::Busy`] when another harness works here; otherwise git
    /// or the file system failed.
    pub fn hold(repo: &Repo) -> Result<Self, RunLockError> {
        let dir = lock_dir(repo)?;
        let Some(held) = try_hold(&dir)? else {
            let task = read_task(&dir);
            let can_stop = task
                .as_deref()
                .is_some_and(|task| is_plain_file(&stop_file(&dir, task)));
            return Err(RunLockError::Busy { task, can_stop });
        };
        // Files a killed run left would name the wrong task, or let
        // `harness stop` think work that cannot stop is a run.
        remove_file_if_there(&dir.join(TASK_FILE))?;
        remove_stop_files(&dir)?;
        Ok(Self {
            dir,
            task: None,
            _held: held,
        })
    }

    /// Says that the work is on `task_id`, for a second harness to read.
    ///
    /// # Errors
    ///
    /// The id is not a task id, or the task file cannot be written.
    pub fn set_task(&mut self, task_id: &str) -> Result<(), RunLockError> {
        check_task_id(task_id)?;
        let task_file = self.dir.join(TASK_FILE);
        fs::write(&task_file, task_id).map_err(|source| io_error(&task_file, source))?;
        self.task = Some(task_id.to_string());
        Ok(())
    }

    /// Lets `harness stop` reach this run: makes the task's empty stop file
    /// and lets `stop` read it. Only for work that runs roles; call it after
    /// [`RunLock::set_task`] (before, there is no task to stop).
    ///
    /// # Errors
    ///
    /// The stop file cannot be made.
    pub fn connect(&self, stop: &StopRequest) -> Result<(), RunLockError> {
        let Some(task) = &self.task else {
            return Ok(());
        };
        let file = stop_file(&self.dir, task);
        // Empty: nothing asked. It also replaces a request a crashed run left.
        remove_file_if_there(&file)?;
        fs::write(&file, "").map_err(|source| io_error(&file, source))?;
        stop.also_read(file);
        Ok(())
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        // Nothing to do if they are already gone; the lock itself is what counts.
        if let Some(task) = &self.task {
            let _ = remove_file_if_there(&stop_file(&self.dir, task));
        }
        let _ = remove_file_if_there(&self.dir.join(TASK_FILE));
    }
}

/// Asks the harness running the roles of `task_id` in this project to stop,
/// `when` as the TUI's buttons: «now» at once, «after this step» once the
/// running role has finished. A «now» asked earlier stays «now».
///
/// # Errors
///
/// [`RunLockError::NotRunning`] when no harness runs that task here, and
/// [`RunLockError::CannotStop`] when it is busy with a step that cannot stop.
pub fn ask_to_stop(repo: &Repo, task_id: &str, when: StopWhen) -> Result<(), RunLockError> {
    check_task_id(task_id)?;
    let dir = lock_dir(repo)?;
    // Free lock: nobody runs. Taking it for this moment is the only sure
    // test (a task file can outlive a crashed run); dropping gives it back.
    if try_hold(&dir)?.is_some() || read_task(&dir).as_deref() != Some(task_id) {
        return Err(RunLockError::NotRunning(task_id.to_string()));
    }
    let file = stop_file(&dir, task_id);
    // Only into the plain file the run made: not through a link an agent
    // put there, which could point at one of the user's files.
    if !is_plain_file(&file) {
        return Err(RunLockError::CannotStop(task_id.to_string()));
    }
    if stop_request::read_stop_file(&file) == Some(StopWhen::Now) {
        return Ok(());
    }
    // A new file renamed over the old one replaces the name itself: nothing
    // is ever written through a link (also a hard link) put in its place.
    let new = dir.join(format!("new-stop-{task_id}"));
    fs::write(&new, when.word()).map_err(|source| io_error(&new, source))?;
    fs::rename(&new, &file).map_err(|source| io_error(&file, source))
}

/// Removes every stop file in `dir`, also those of other tasks.
fn remove_stop_files(dir: &Path) -> Result<(), RunLockError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error(dir, source)),
    };
    for entry in entries {
        let entry = entry.map_err(|source| io_error(dir, source))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("stop-") || name.starts_with("new-stop-") {
            remove_file_if_there(&entry.path())?;
        }
    }
    Ok(())
}

/// Where the output of a `harness run` of `task_id` started in the background
/// goes (by the MCP server): `.git/harness/run-<task>.log`, out of git's view.
///
/// # Errors
///
/// The id is not a task id, or git could not say where its folder is.
pub fn background_log(repo: &Repo, task_id: &str) -> Result<PathBuf, RunLockError> {
    check_task_id(task_id)?;
    Ok(lock_dir(repo)?.join(format!("run-{task_id}.log")))
}

/// `.git/harness` of this working folder.
fn lock_dir(repo: &Repo) -> Result<PathBuf, RunLockError> {
    Ok(repo.git_path(LOCK_DIR)?)
}

/// The stop file of `task` in the lock folder.
fn stop_file(dir: &Path, task: &str) -> PathBuf {
    dir.join(format!("stop-{task}"))
}

/// Takes the lock in `dir`; `None` when another program holds it.
fn try_hold(dir: &Path) -> Result<Option<Held>, RunLockError> {
    let file = dir.join(LOCK_FILE);
    lock::try_hold(&file).map_err(|source| io_error(&file, source))
}

/// The task in the task file; `None` before the holder has written it, or
/// when the file holds anything but a task id (it is shown to the user).
fn read_task(dir: &Path) -> Option<String> {
    let text = stop_request::read_small_file(&dir.join(TASK_FILE))?;
    let task = text.trim();
    check_task_id(task).is_ok().then(|| task.to_string())
}

/// Is `path` a plain file (not a link, folder or pipe)?
fn is_plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file())
}

/// Removes the file `path`; one that is not there is fine.
fn remove_file_if_there(path: &Path) -> Result<(), RunLockError> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other.map_err(|source| io_error(path, source)),
    }
}

fn io_error(path: &Path, source: io::Error) -> RunLockError {
    RunLockError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        (dir, repo)
    }

    /// The lock of a run of `task` that runs roles, with its stop request.
    fn running(repo: &Repo, task: &str) -> (RunLock, StopRequest) {
        let mut lock = RunLock::hold(repo).unwrap();
        lock.set_task(task).unwrap();
        let stop = StopRequest::new();
        lock.connect(&stop).unwrap();
        (lock, stop)
    }

    #[test]
    fn a_second_harness_is_told_which_task_the_first_works_on() {
        let (_dir, repo) = repo();
        let mut first = RunLock::hold(&repo).unwrap();
        let unnamed = RunLock::hold(&repo).unwrap_err();
        assert!(matches!(unnamed, RunLockError::Busy { task: None, .. }));

        first.set_task("task-007").unwrap();
        let deciding = RunLock::hold(&repo).unwrap_err().to_string();
        assert!(deciding.contains("working on task-007"), "{deciding}");
        assert!(!deciding.contains("harness stop"), "{deciding}");

        first.connect(&StopRequest::new()).unwrap();
        let running = RunLock::hold(&repo).unwrap_err().to_string();
        assert!(running.contains("harness stop task-007"), "{running}");
    }

    #[test]
    fn the_lock_is_free_again_once_dropped() {
        let (_dir, repo) = repo();
        drop(running(&repo, "task-001"));

        let refused = ask_to_stop(&repo, "task-001", StopWhen::Now).unwrap_err();
        assert!(matches!(refused, RunLockError::NotRunning(_)));
        RunLock::hold(&repo).unwrap();
    }

    #[test]
    fn the_files_live_in_git_s_folder_and_git_does_not_show_them() {
        let (dir, repo) = repo();
        let _run = running(&repo, "task-001");
        ask_to_stop(&repo, "task-001", StopWhen::AfterStep).unwrap();

        assert!(dir.path().join(".git/harness/run.lock").exists());
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn harness_stop_reaches_the_running_harness() {
        let (_dir, repo) = repo();
        let (_lock, stop) = running(&repo, "task-001");

        ask_to_stop(&repo, "task-001", StopWhen::AfterStep).unwrap();
        assert_eq!(stop.asked(), Some(StopWhen::AfterStep));

        ask_to_stop(&repo, "task-001", StopWhen::Now).unwrap();
        assert!(stop.now());
        // A later «after this step» does not take «now» back.
        ask_to_stop(&repo, "task-001", StopWhen::AfterStep).unwrap();
        assert!(stop.now());
    }

    #[test]
    fn stopping_a_task_that_does_not_run_is_refused() {
        let (_dir, repo) = repo();
        let refused = ask_to_stop(&repo, "task-001", StopWhen::Now).unwrap_err();
        assert!(matches!(refused, RunLockError::NotRunning(_)));

        let _run = running(&repo, "task-002");
        let other = ask_to_stop(&repo, "task-001", StopWhen::Now).unwrap_err();
        assert!(matches!(other, RunLockError::NotRunning(_)));
        let bad = ask_to_stop(&repo, "../task-002", StopWhen::Now).unwrap_err();
        assert!(matches!(bad, RunLockError::BadTaskId(_)));
    }

    #[test]
    fn work_that_cannot_stop_halfway_says_so() {
        let (_dir, repo) = repo();
        // A decision or a merge: the lock is held, but no stop file is made.
        let mut lock = RunLock::hold(&repo).unwrap();
        lock.set_task("task-001").unwrap();

        let refused = ask_to_stop(&repo, "task-001", StopWhen::Now).unwrap_err();

        assert!(matches!(refused, RunLockError::CannotStop(_)));
    }

    #[test]
    fn a_late_request_for_one_task_does_not_stop_the_next() {
        let (_dir, repo) = repo();
        let dir = lock_dir(&repo).unwrap();
        // A request for task-001 that lands after its run has ended.
        fs::create_dir_all(&dir).unwrap();
        fs::write(stop_file(&dir, "task-001"), "now").unwrap();

        let (_lock, stop) = running(&repo, "task-002");

        assert_eq!(stop.asked(), None);
    }

    #[test]
    fn an_old_request_for_the_same_task_does_not_stop_its_next_run() {
        let (_dir, repo) = repo();
        let dir = lock_dir(&repo).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(stop_file(&dir, "task-001"), "now").unwrap();

        let (_lock, stop) = running(&repo, "task-001");

        assert_eq!(stop.asked(), None);
    }

    #[test]
    fn a_stop_file_a_killed_run_left_does_not_make_a_decision_stoppable() {
        let (_dir, repo) = repo();
        let dir = lock_dir(&repo).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(stop_file(&dir, "task-001"), "").unwrap();

        // `harness approve task-001`: the lock, but no stop file of its own.
        let mut lock = RunLock::hold(&repo).unwrap();
        lock.set_task("task-001").unwrap();

        let refused = ask_to_stop(&repo, "task-001", StopWhen::AfterStep).unwrap_err();
        assert!(matches!(refused, RunLockError::CannotStop(_)));
    }

    #[test]
    fn a_task_file_with_other_text_names_no_task() {
        let (_dir, repo) = repo();
        let lock = RunLock::hold(&repo).unwrap();
        fs::write(lock.dir.join(TASK_FILE), "\u{1b}[2Jtask-001").unwrap();

        let busy = RunLock::hold(&repo).unwrap_err();

        assert!(matches!(busy, RunLockError::Busy { task: None, .. }));
    }

    #[cfg(unix)]
    #[test]
    fn harness_stop_does_not_write_through_a_link() {
        let (dir, repo) = repo();
        let (lock, _stop) = running(&repo, "task-001");
        let users_file = dir.path().join("bashrc");
        fs::write(&users_file, "mine").unwrap();
        let file = stop_file(&lock.dir, "task-001");
        fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(&users_file, &file).unwrap();

        let refused = ask_to_stop(&repo, "task-001", StopWhen::Now).unwrap_err();

        assert!(matches!(refused, RunLockError::CannotStop(_)));
        assert_eq!(fs::read_to_string(&users_file).unwrap(), "mine");
    }

    #[test]
    fn harness_stop_does_not_write_through_a_hard_link() {
        let (dir, repo) = repo();
        let (lock, _stop) = running(&repo, "task-001");
        let users_file = dir.path().join("bashrc");
        fs::write(&users_file, "mine").unwrap();
        let file = stop_file(&lock.dir, "task-001");
        fs::remove_file(&file).unwrap();
        fs::hard_link(&users_file, &file).unwrap();

        ask_to_stop(&repo, "task-001", StopWhen::Now).unwrap();

        assert_eq!(fs::read_to_string(&users_file).unwrap(), "mine");
        assert_eq!(fs::read_to_string(&file).unwrap(), "now");
    }
}
