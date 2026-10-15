//! Finding every process one agent run started, even the ones that left its
//! process group. Codex, for example, starts each shell command in a process
//! group of its own, so a server it leaves in the background survives
//! [`crate::process::kill_tree_of`].
//!
//! The run gets a mark in its environment ([`mark`]); every process it starts
//! inherits it, whatever group or session it moves to. [`kill_marked`] stops
//! all processes that carry the mark.
//!
//! - Linux: the mark is read from `/proc/<pid>/environ`.
//! - macOS: from `ps -E`, which shows the environment of the user's own
//!   processes.
//! - Windows: not supported yet; there [`kill_marked`] does nothing.
//!
//! A program that clears its children's environment drops the mark; its
//! children are then found only through the process group, as before.

use std::process::{Command, Stdio};

/// The environment variable that carries the mark. It holds no secret.
pub const VARIABLE: &str = "HARNESS_RUN_MARK";

/// How many times [`kill_marked`] looks again: a process can start a child
/// between the look and the kill, so one pass may miss it.
const PASSES: usize = 5;

/// Gives `command` the mark `mark`, so [`kill_marked`] finds it and every
/// process it starts.
pub fn mark<'a>(command: &'a mut Command, mark: &str) -> &'a mut Command {
    command.env(VARIABLE, mark)
}

/// Stops every process of this user that carries the mark `mark`. A process
/// that has already ended, or that cannot be read, is skipped.
pub fn kill_marked(mark: &str) {
    let entry = format!("{VARIABLE}={mark}");
    for _ in 0..PASSES {
        let pids = marked_pids(&entry);
        if pids.is_empty() {
            return;
        }
        kill(&pids);
    }
}

/// The ids of the processes whose environment holds `entry` (`NAME=value`).
#[cfg(target_os = "linux")]
fn marked_pids(entry: &str) -> Vec<u32> {
    let Ok(folders) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    folders
        .filter_map(|folder| folder.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            // Another user's process cannot be read: that is not ours.
            std::fs::read(format!("/proc/{pid}/environ"))
                .is_ok_and(|environ| environ.split(|&b| b == 0).any(|v| v == entry.as_bytes()))
        })
        .collect()
}

/// The ids of the processes whose environment holds `entry` (`NAME=value`).
#[cfg(target_os = "macos")]
fn marked_pids(entry: &str) -> Vec<u32> {
    // `-E` adds the environment after the command, separated by spaces.
    let Ok(output) = Command::new("ps")
        .args(["-A", "-ww", "-E", "-o", "pid=,command="])
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.split(' ').any(|word| word == entry))
        .filter_map(|line| line.split_whitespace().next()?.parse().ok())
        .collect()
}

/// Windows is not supported yet: no process is found.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn marked_pids(_entry: &str) -> Vec<u32> {
    Vec::new()
}

/// Stops the processes `pids` at once.
fn kill(pids: &[u32]) {
    let pids: Vec<String> = pids.iter().map(u32::to_string).collect();
    let _ = Command::new("kill")
        .args(["-KILL", "--"])
        .args(&pids)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Does process `pid` still run? A stopped process can take a moment to
    /// disappear, so this waits a little. A stopped process nobody has reaped
    /// yet (state `Z`) does not count: it no longer runs.
    fn still_running(pid: &str) -> bool {
        for _ in 0..40 {
            let output = Command::new("ps")
                .args(["-o", "stat=", "-p", pid])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&output.stdout);
            let state = state.trim();
            if state.is_empty() || state.starts_with('Z') {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        true
    }

    /// Waits until process `pid` leads a process group of its own.
    fn wait_for_own_group(pid: &str) {
        for _ in 0..100 {
            let output = Command::new("ps")
                .args(["-o", "pgid=", "-p", pid])
                .output()
                .unwrap();
            if String::from_utf8_lossy(&output.stdout).trim() == pid {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("process {pid} never left the group");
    }

    #[test]
    fn a_process_that_left_the_group_is_stopped_by_its_mark() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        // The background `sleep` moves to a process group of its own (perl's
        // `setpgrp`), as Codex does with its commands; the shell ends at once.
        let script = format!(
            "perl -e 'setpgrp(0, 0); exec @ARGV' sleep 30 & echo $! > '{}'",
            pid_file.display()
        );
        let mut command = Command::new("sh");
        command.args(["-c", &script]);
        mark(&mut command, "test-mark-1");
        let mut shell = crate::process::own_group(&mut command).spawn().unwrap();
        shell.wait().unwrap();
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let pid = pid.trim().to_string();
        // Wait until the sleep has left the shell's group; stopping that
        // group then misses it.
        wait_for_own_group(&pid);
        crate::process::kill_tree_of(shell.id());
        assert!(still_running(&pid));

        kill_marked("test-mark-2");
        assert!(still_running(&pid), "another mark must not stop it");
        kill_marked("test-mark-1");

        assert!(!still_running(&pid));
    }
}
