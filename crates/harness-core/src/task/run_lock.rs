//! One harness at a time in a project folder: the run lock, and the stop file
//! through which another program asks the running harness to stop.
//!
//! Two programs working in one folder at once (the TUI and `harness run` in a
//! terminal, or two terminals) would switch branches and commit under each
//! other's feet. So everything that changes a task (create, decide, run)
//! first holds the lock `.git/harness/run.lock`; a second one is refused and
//! told which task the first is busy with (`run.task` next to it).
//!
//! The files live in git's own folder, not in `.harness/`: git never shows
//! them as changes, they stay when a branch is switched, and agents are not
//! allowed to change `.git`. The operating system holds the lock (see
//! `harness_platform::lock`), so a crashed run never leaves it behind.
//!
//! `harness stop` (and later the MCP server) writes `run.stop`; the running
//! harness reads it through its [`StopRequest`] and stops exactly as if the
//! user had pressed the button in the TUI.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use harness_platform::lock::{self, Held};

use crate::git::{GitError, Repo};
use crate::task::stop_request::{StopRequest, StopWhen};

/// The folder inside `.git` with the lock and its two small files.
const LOCK_DIR: &str = "harness";

/// The file the running harness holds the lock on.
const LOCK_FILE: &str = "run.lock";

/// The task the running harness works on, for the message to a second one.
const TASK_FILE: &str = "run.task";

/// The request to stop, written by another program.
const STOP_FILE: &str = "run.stop";

/// Why the lock could not be taken or a stop could not be asked.
#[derive(Debug, thiserror::Error)]
pub enum RunLockError {
    /// Another harness works in this project folder right now.
    #[error("{}", busy_text(.task.as_deref()))]
    Busy {
        /// The task it works on, when it has said so.
        task: Option<String>,
    },
    /// No harness works in this project folder, so there is nothing to stop.
    #[error("no harness is working on {0} in this project right now")]
    NotRunning(String),
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
fn busy_text(task: Option<&str>) -> String {
    match task {
        Some(task) => format!(
            "another harness is working on {task} in this project; wait until it \
             finishes, or stop it with: harness stop {task}"
        ),
        None => "another harness is working in this project; wait until it finishes".into(),
    }
}

/// The held lock of this project folder. Dropping it lets the next harness in.
#[derive(Debug)]
pub struct RunLock {
    dir: PathBuf,
    // Declared last, so it is dropped after `Drop::drop` has removed the
    // small files: nobody can take the lock and find our old ones.
    _held: Held,
}

impl RunLock {
    /// Takes the lock for work on `task_id`, or says who holds it.
    ///
    /// # Errors
    ///
    /// [`RunLockError::Busy`] when another harness works here; otherwise git
    /// or the file system failed.
    pub fn hold(repo: &Repo, task_id: &str) -> Result<Self, RunLockError> {
        let dir = lock_dir(repo)?;
        let Some(held) = try_hold(&dir)? else {
            return Err(RunLockError::Busy {
                task: read_task(&dir),
            });
        };
        // A stop asked of an earlier run that ended before it read it is old.
        remove_if_there(&dir.join(STOP_FILE)).map_err(|source| io_error(&dir, source))?;
        let lock = Self { dir, _held: held };
        lock.set_task(task_id)?;
        Ok(lock)
    }

    /// Says that the work is now on `task_id`: a new task gets its id only
    /// after the lock is taken.
    ///
    /// # Errors
    ///
    /// The task file cannot be written.
    pub fn set_task(&self, task_id: &str) -> Result<(), RunLockError> {
        let task_file = self.dir.join(TASK_FILE);
        fs::write(&task_file, task_id).map_err(|source| io_error(&task_file, source))
    }

    /// Lets `stop` also read this run's stop file, so `harness stop` reaches it.
    pub fn connect(&self, stop: &StopRequest) {
        stop.also_read(self.dir.join(STOP_FILE));
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        // Nothing to do if they are already gone; the lock itself is what counts.
        let _ = remove_if_there(&self.dir.join(STOP_FILE));
        let _ = remove_if_there(&self.dir.join(TASK_FILE));
    }
}

/// Asks the harness working on `task_id` in this project to stop, `when` as
/// the TUI's buttons. It stops within a second or so, not before this returns.
///
/// # Errors
///
/// [`RunLockError::NotRunning`] when no harness works on that task here.
pub fn ask_to_stop(repo: &Repo, task_id: &str, when: StopWhen) -> Result<(), RunLockError> {
    let dir = lock_dir(repo)?;
    if running_task(&dir)?.as_deref() != Some(task_id) {
        return Err(RunLockError::NotRunning(task_id.to_string()));
    }
    let file = dir.join(STOP_FILE);
    fs::write(&file, when.word()).map_err(|source| io_error(&file, source))
}

/// The task a harness works on in this project right now, if any.
///
/// # Errors
///
/// Git or the file system failed.
pub fn running(repo: &Repo) -> Result<Option<String>, RunLockError> {
    running_task(&lock_dir(repo)?)
}

/// The task the holder of the lock in `dir` works on. Taking the lock for
/// a moment is the only sure test: a task file can outlive a crashed run.
fn running_task(dir: &Path) -> Result<Option<String>, RunLockError> {
    match try_hold(dir)? {
        // Free: nobody runs. Dropping `held` gives it back at once.
        Some(held) => {
            drop(held);
            Ok(None)
        }
        None => Ok(read_task(dir)),
    }
}

/// `.git/harness` of this working folder.
fn lock_dir(repo: &Repo) -> Result<PathBuf, RunLockError> {
    Ok(repo.git_path(LOCK_DIR)?)
}

/// Takes the lock in `dir`; `None` when another program holds it.
fn try_hold(dir: &Path) -> Result<Option<Held>, RunLockError> {
    let file = dir.join(LOCK_FILE);
    lock::try_hold(&file).map_err(|source| io_error(&file, source))
}

/// The task in the task file; `None` before the holder has written it.
fn read_task(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join(TASK_FILE)).ok()?;
    let task = text.trim();
    (!task.is_empty()).then(|| task.to_string())
}

/// Removes `path`; a file that is not there is fine.
fn remove_if_there(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
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

    #[test]
    fn a_second_harness_is_told_which_task_the_first_works_on() {
        let (_dir, repo) = repo();
        let first = RunLock::hold(&repo, "task-007").unwrap();

        let second = RunLock::hold(&repo, "task-008").unwrap_err();

        assert!(matches!(&second, RunLockError::Busy { task: Some(t) } if t == "task-007"));
        assert!(second.to_string().contains("harness stop task-007"));
        assert_eq!(running(&repo).unwrap().as_deref(), Some("task-007"));
        drop(first);
    }

    #[test]
    fn a_new_task_is_named_once_it_has_its_id() {
        let (_dir, repo) = repo();
        let held = RunLock::hold(&repo, "a new task").unwrap();

        held.set_task("task-009").unwrap();

        assert_eq!(running(&repo).unwrap().as_deref(), Some("task-009"));
        ask_to_stop(&repo, "task-009", StopWhen::AfterStep).unwrap();
    }

    #[test]
    fn the_lock_is_free_again_once_dropped() {
        let (_dir, repo) = repo();
        drop(RunLock::hold(&repo, "task-001").unwrap());

        assert_eq!(running(&repo).unwrap(), None);
        RunLock::hold(&repo, "task-002").unwrap();
    }

    #[test]
    fn the_files_live_in_git_s_folder_and_git_does_not_show_them() {
        let (dir, repo) = repo();
        let held = RunLock::hold(&repo, "task-001").unwrap();
        ask_to_stop(&repo, "task-001", StopWhen::AfterStep).unwrap();

        assert!(dir.path().join(".git/harness/run.lock").exists());
        assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
        drop(held);
    }

    #[test]
    fn harness_stop_reaches_the_running_harness() {
        let (_dir, repo) = repo();
        let held = RunLock::hold(&repo, "task-001").unwrap();
        let stop = StopRequest::new();
        held.connect(&stop);

        ask_to_stop(&repo, "task-001", StopWhen::Now).unwrap();

        assert!(stop.now());
    }

    #[test]
    fn stopping_a_task_that_does_not_run_is_refused() {
        let (_dir, repo) = repo();
        let refused = ask_to_stop(&repo, "task-001", StopWhen::Now).unwrap_err();
        assert!(matches!(refused, RunLockError::NotRunning(_)));

        let _held = RunLock::hold(&repo, "task-002").unwrap();
        let other = ask_to_stop(&repo, "task-001", StopWhen::Now).unwrap_err();
        assert!(matches!(other, RunLockError::NotRunning(_)));
    }

    #[test]
    fn an_old_stop_request_does_not_stop_the_next_run() {
        let (_dir, repo) = repo();
        // A run killed before it could clean up leaves its stop file behind.
        let dir = lock_dir(&repo).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(STOP_FILE), "now").unwrap();

        let held = RunLock::hold(&repo, "task-001").unwrap();
        let stop = StopRequest::new();
        held.connect(&stop);

        assert_eq!(stop.asked(), None);
    }
}
