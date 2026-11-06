//! A lock file that only one running program can hold at a time: so a second
//! TUI window does not open while the first one is still running, and so two
//! windows change a shared file in `~/.harness` one after the other.
//!
//! The operating system keeps the lock, not the file: it ends when the holder
//! closes the file or ends in any way (a crash, `kill`, a closed terminal),
//! so a stale lock never has to be cleaned up by hand. The file may stay on
//! disk; that is fine, only the lock on it counts.
//!
//! Linux and macOS use `flock`, Windows `LockFileEx` (both behind
//! [`std::fs::File::try_lock`]). Rust opens files so that child programs do
//! not inherit them, so an agent the holder started never keeps the lock.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

/// A held lock; it is given back when this value is dropped.
#[derive(Debug)]
pub struct Held {
    // The lock lives as long as the open file: dropping the file closes it,
    // and the system lets the lock go.
    _file: File,
}

/// Takes the lock on the file `path`, making the file and its folder if
/// needed. Returns `None` when another program (or another `Held` in this
/// one) already holds it.
///
/// # Errors
///
/// The folder or file cannot be made or opened, or the system cannot lock it.
pub fn try_hold(path: &Path) -> io::Result<Option<Held>> {
    let file = open_lock_file(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(Held { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// How long [`try_hold_for`] sleeps between two tries.
const RETRY_PAUSE: Duration = Duration::from_millis(50);

/// Like [`try_hold`], but tries again for up to `patience` before it gives up.
///
/// A program being started holds a copy of every open file of the program
/// that starts it, this lock included, until it has really begun running
/// (Rust closes those copies then). So while the harness starts an agent or
/// git in another thread, a free lock can look held for a few milliseconds.
///
/// # Errors
///
/// The folder or file cannot be made or opened, or the system cannot lock it.
pub fn try_hold_for(path: &Path, patience: Duration) -> io::Result<Option<Held>> {
    let started = Instant::now();
    loop {
        if let Some(held) = try_hold(path)? {
            return Ok(Some(held));
        }
        if started.elapsed() >= patience {
            return Ok(None);
        }
        std::thread::sleep(RETRY_PAUSE);
    }
}

/// Waits until nobody holds the lock on the file `path`, then takes it,
/// making the file and its folder if needed.
///
/// Only for short work, such as reading, changing and writing one small
/// file: whoever waits here cannot do anything else meanwhile.
///
/// # Errors
///
/// The folder or file cannot be made or opened, or the system cannot lock it.
pub fn wait_and_hold(path: &Path) -> io::Result<Held> {
    let file = open_lock_file(path)?;
    file.lock()?;
    Ok(Held { _file: file })
}

/// Opens (or makes) the lock file, and its folder.
fn open_lock_file(path: &Path) -> io::Result<File> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    // `truncate(false)`: the file holds nothing, but opening it must not
    // change it while another program holds the lock.
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_holder_waits_until_the_first_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("folder").join("tui.lock");

        let first = try_hold(&path).unwrap();
        assert!(first.is_some(), "a free lock is taken");
        assert!(try_hold(&path).unwrap().is_none(), "a held lock is not");

        drop(first);
        assert!(hold_soon(&path).is_some(), "a dropped lock is free");
    }

    /// Takes the lock, trying again for a moment. Other tests start programs
    /// at the same time, and a program being started holds a copy of every
    /// open file (this lock included) until it has really begun running;
    /// only then are those copies closed.
    fn hold_soon(path: &Path) -> Option<Held> {
        for _ in 0..40 {
            if let Some(held) = try_hold(path).unwrap() {
                return Some(held);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        None
    }

    #[test]
    fn a_patient_holder_gets_a_lock_let_go_meanwhile_and_gives_up_on_a_held_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalogs.lock");
        let first = try_hold(&path).unwrap();
        let patience = Duration::from_millis(200);

        assert!(
            try_hold_for(&path, patience).unwrap().is_none(),
            "still held"
        );
        let letting_go = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(first);
        });
        let second = try_hold_for(&path, Duration::from_secs(5)).unwrap();
        letting_go.join().unwrap();

        assert!(second.is_some(), "let go while waiting");
    }

    #[test]
    fn a_waiting_holder_gets_the_lock_when_the_first_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects.toml.lock");
        let first = wait_and_hold(&path).unwrap();
        let waiter_path = path.clone();

        let waiter = std::thread::spawn(move || wait_and_hold(&waiter_path).map(|_held| ()));
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!waiter.is_finished(), "the lock is still held");
        drop(first);

        waiter.join().unwrap().unwrap();
    }

    /// Tells [`hold_until_killed`] which lock to take: set only in the child
    /// program that [`a_killed_holder_leaves_no_lock`] starts.
    const HOLDER_PATH: &str = "HARNESS_TEST_LOCK_HOLDER";

    /// The child program of [`a_killed_holder_leaves_no_lock`]: this test
    /// program run again for this one test. Run normally it does nothing.
    #[test]
    fn hold_until_killed() {
        let Some(path) = std::env::var_os(HOLDER_PATH) else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        let _held = try_hold(&path).unwrap().expect("the lock is free");
        fs::write(path.with_extension("ready"), "").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    #[test]
    fn a_killed_holder_leaves_no_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tui.lock");
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lock::tests::hold_until_killed", "--nocapture"])
            .env(HOLDER_PATH, &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // Killed also when an assert below fails, so it never lingers.
        let mut child = KillOnDrop(child);
        let ready = path.with_extension("ready");
        for _ in 0..200 {
            if ready.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(ready.exists(), "the child did not take the lock");
        assert!(try_hold(&path).unwrap().is_none(), "the child holds it");

        // `kill -9` on Linux and macOS, `TerminateProcess` on Windows: the
        // child gets no chance to clean anything up.
        child.0.kill().unwrap();
        child.0.wait().unwrap();

        assert!(hold_soon(&path).is_some(), "the system let the lock go");
    }

    /// A child program that is killed when the test ends, however it ends.
    struct KillOnDrop(std::process::Child);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn a_child_program_does_not_keep_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tui.lock");
        let program = harness_fake::install(dir.path(), "agent", "sleep 30\n");
        let held = try_hold(&path).unwrap();
        // Started while the lock is held, as the TUI starts its agents.
        let mut child = std::process::Command::new(program).spawn().unwrap();

        drop(held);
        let again = hold_soon(&path);

        child.kill().unwrap();
        child.wait().unwrap();
        assert!(again.is_some(), "the still running child must not hold it");
    }
}
