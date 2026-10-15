//! Finding every process one agent run started, even the ones that left its
//! process group. Codex, for example, starts each shell command in a process
//! group of its own, so a server it leaves in the background survives
//! [`crate::process::kill_tree_of`].
//!
//! The run gets a mark in its environment ([`new_mark`], [`mark`]); every
//! process it starts inherits it, whatever group or session it moves to.
//! [`kill_marked`] stops all processes of this user that carry the mark.
//!
//! - Linux: the mark is read from `/proc/<pid>/environ`, and each process is
//!   held by a pidfd from the check to the kill, so a process id that was
//!   reused meanwhile is never hit. When a pidfd cannot be opened (a kernel
//!   before 5.3, a strict container, too many open files) the process gets
//!   a plain kill right after the check, with the macOS risk below.
//! - macOS: the mark is read from `ps -E`. Between that look and `kill` a
//!   process can end and its id be reused; the window is short, but it exists.
//! - Other systems (Windows): not supported yet; [`kill_marked`] does
//!   nothing, and a helper whose parent has ended is still missed.
//!
//! Limits. The mark is only a strong hint, not a sandbox:
//! - a program can lose it: clear its children's environment (`env -i`; the
//!   MCP servers Codex starts get a cleared one), overwrite it while setting
//!   its process title (`postgres`, `nginx`, Perl's `$0 =`), start through a
//!   service manager (`systemd-run`, `launchctl`, `docker`); an agent can
//!   also remove it on purpose;
//! - a process whose environment cannot be read is skipped: another user's,
//!   `sudo`, or one that forbids it (`ssh-agent`, `gpg-agent`);
//! - a harness started inside a run replaces the mark with its own, so its
//!   agents are not found through the outer mark;
//! - a shared server the run started (a build daemon, a tmux server) carries
//!   the mark, and is stopped with the run even if something else uses it;
//! - reading the environment of a process stuck on a hung network drive can
//!   block, and the harness waits with it.

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The environment variable that carries the mark. It holds no secret.
pub const VARIABLE: &str = "HARNESS_RUN_MARK";

/// How many times [`kill_marked`] looks again: a process can start a child
/// between the look and the kill, so one pass may miss it.
const PASSES: usize = 5;

/// A mark no other run has: this program's id, the time and a count.
pub fn new_mark() -> String {
    // The count is the one piece of state shared by all runs of this
    // program: two runs started in the same instant (tests in parallel)
    // still get different marks.
    static RUNS: AtomicU64 = AtomicU64::new(0);
    let run = RUNS.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    format!("{}-{nanos}-{run}", std::process::id())
}

/// Gives `command` the mark `mark`, so [`kill_marked`] finds it and every
/// process it starts.
pub fn mark<'a>(command: &'a mut Command, mark: &str) -> &'a mut Command {
    command.env(VARIABLE, mark)
}

/// Stops every process this program can read that carries the mark `mark`.
/// A process that has already ended, or that cannot be read, is skipped.
///
/// It looks at most five times (a process can start a child between a look
/// and the kill) and reports nothing. On macOS it runs `ps` and `kill`; on
/// Windows it does nothing yet. It blocks while it reads the processes.
pub fn kill_marked(mark: &str) {
    let entry = format!("{VARIABLE}={mark}");
    for _ in 0..PASSES {
        if kill_pass(&entry) == 0 {
            return;
        }
    }
}

/// One look at all processes: stops those whose environment holds `entry`
/// (`NAME=value`) and returns how many it stopped.
#[cfg(target_os = "linux")]
fn kill_pass(entry: &str) -> usize {
    use rustix::process::{kill_process, pidfd_open, pidfd_send_signal, Pid, PidfdFlags, Signal};

    let Ok(folders) = std::fs::read_dir("/proc") else {
        return 0;
    };
    let mut stopped = 0;
    for folder in folders.flatten() {
        let Some(pid) = folder.file_name().to_str().and_then(|n| n.parse().ok()) else {
            continue;
        };
        let Some(pid) = Pid::from_raw(pid) else {
            continue;
        };
        // Hold the process first, then check it: if the id was reused
        // before `pidfd_open`, the check below reads the new process, which
        // has no mark, and nothing is sent.
        let handle = pidfd_open(pid, PidfdFlags::empty());
        // Another user's process cannot be read: that is not ours.
        let marked = std::fs::read(format!("/proc/{pid}/environ"))
            .is_ok_and(|environ| environ.split(|&b| b == 0).any(|v| v == entry.as_bytes()));
        if !marked {
            continue;
        }
        let killed = match &handle {
            Ok(handle) => pidfd_send_signal(handle, Signal::KILL),
            // An old kernel without pidfd: a plain kill right after the check.
            Err(_) => kill_process(pid, Signal::KILL),
        };
        if killed.is_ok() {
            stopped += 1;
        }
    }
    stopped
}

/// One look at all processes: stops those whose environment holds `entry`
/// (`NAME=value`) and returns how many it stopped.
#[cfg(target_os = "macos")]
fn kill_pass(entry: &str) -> usize {
    use std::process::Stdio;

    // `-E` adds the environment after the command, separated by spaces.
    let Ok(output) = Command::new("ps")
        .args(["-A", "-ww", "-E", "-o", "pid=,command="])
        .stderr(Stdio::null())
        .output()
    else {
        return 0;
    };
    let pids: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.split(' ').any(|word| word == entry))
        .filter_map(|line| Some(line.split_whitespace().next()?.to_string()))
        .collect();
    if pids.is_empty() {
        return 0;
    }
    let _ = Command::new("kill")
        .args(["-KILL", "--"])
        .args(&pids)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    pids.len()
}

/// Other systems (Windows) are not supported yet: nothing is found.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn kill_pass(_entry: &str) -> usize {
    0
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

    /// Does process `pid` lead a process group of its own?
    fn in_own_group(pid: &str) -> bool {
        let output = Command::new("ps")
            .args(["-o", "pgid=", "-p", pid])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim() == pid
    }

    #[test]
    fn a_process_that_left_the_group_is_stopped_by_its_mark() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        // The fake starts a sleeping helper in a process group of its own,
        // as Codex does with its commands, and ends at once.
        let script = format!("spawn-sleep-alone 30 {}\n", pid_file.display());
        let program = harness_fake::install(dir.path(), "agent", &script);
        // Fresh marks: another test run on this machine has others.
        let ours = new_mark();
        let other = new_mark();
        let mut command = Command::new(program);
        mark(&mut command, &ours);
        crate::process::own_group(&mut command).status().unwrap();
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let pid = pid.trim().to_string();
        assert!(in_own_group(&pid));

        kill_marked(&other);
        assert!(still_running(&pid), "another mark must not stop it");
        kill_marked(&ours);

        assert!(!still_running(&pid));
    }

    // macOS may hide the environment of its own system programs; Codex runs
    // exactly those (`/bin/sh`, `/bin/sleep`), so check one of them there.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_system_program_that_left_the_group_is_stopped_by_its_mark() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let script = format!(
            "perl -e 'setpgrp(0, 0); exec @ARGV' /bin/sleep 30 & echo $! > '{}'",
            pid_file.display()
        );
        let ours = new_mark();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", &script]);
        mark(&mut command, &ours);
        command.status().unwrap();
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let pid = pid.trim().to_string();

        kill_marked(&ours);

        assert!(!still_running(&pid));
    }

    #[test]
    fn every_mark_is_new() {
        assert_ne!(new_mark(), new_mark());
    }
}
