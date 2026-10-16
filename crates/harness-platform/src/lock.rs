//! A lock file that only one running program can hold at a time, so a second
//! TUI window does not open while the first one is still running.
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
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    // `truncate(false)`: the file holds nothing, but opening it must not
    // change it while another program holds the lock.
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(Held { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
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
        assert!(try_hold(&path).unwrap().is_some(), "a dropped lock is free");
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
        let again = try_hold(&path).unwrap();

        child.kill().unwrap();
        child.wait().unwrap();
        assert!(again.is_some(), "the still running child must not hold it");
    }
}
