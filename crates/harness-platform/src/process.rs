//! Stopping a program together with everything it started. `npx`, for
//! example, starts the real MCP server as its own child; stopping only `npx`
//! would leave the server running.
//!
//! On Linux and macOS the program gets its own process group, and the whole
//! group is stopped. On Windows `taskkill /T` stops the program and every
//! process it started. [`detached`] is the opposite: a program that keeps
//! running after its starter has ended.

use std::io;
use std::process::{Child, Command, Stdio};

/// Prepares `command` so that [`kill_tree`] can stop everything it starts.
pub fn own_group(command: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

/// Windows: the started program gets no console of its own and does not
/// share the starter's, so closing the starter's window does not stop it.
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;

/// Windows: the started program does not get the Ctrl+C sent to the starter.
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Prepares `command` to keep running after the program that starts it has
/// ended, like `setsid nohup` in a shell: its own process group (Linux,
/// macOS) or no console shared with the starter (Windows), and no input.
/// Its output should go to a file; the starter's output may be gone.
pub fn detached(command: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command.stdin(Stdio::null())
}

/// Stops `child` and everything it started (for a child started with
/// [`own_group`]), then waits for it so no finished process is left behind.
pub fn kill_tree(child: &mut Child) {
    kill_tree_of(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

/// Stops the program with process id `pid` and everything it started (for a
/// program started with [`own_group`]). Unlike [`kill_tree`] it needs only the
/// id, so it also works for programs started through `tokio`. A program that
/// has already ended is not an error.
///
/// On Linux and macOS this stops the whole process group, even the processes
/// whose parent has already ended. On Windows `taskkill /T` follows the
/// parent-child links, so it finds only the processes still linked to `pid`.
pub fn kill_tree_of(pid: u32) {
    let pid = pid.to_string();
    #[cfg(unix)]
    let mut killer = {
        let mut killer = Command::new("kill");
        // `-pid` means "the process group pid", not one process.
        killer.args(["-KILL", "--", &format!("-{pid}")]);
        killer
    };
    #[cfg(windows)]
    let mut killer = {
        let mut killer = Command::new("taskkill");
        killer.args(["/T", "/F", "/PID", &pid]);
        killer
    };
    let _ = killer
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Turns this program into `command`, keeping its standard input and output.
/// Returns only on an error.
///
/// On Linux and macOS the program is replaced (`exec`), so nothing of the
/// harness is left running. Windows has no `exec`: there `command` runs as a
/// child, and the harness ends with the child's exit code when it finishes.
pub fn replace_with(command: &mut Command) -> io::Error {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.exec()
    }
    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) => e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read};
    use std::time::{Duration, Instant};

    /// A program that starts a long-running child of its own. The child
    /// itself prints a first line, so a test can wait until it really runs.
    fn parent_with_a_child() -> Command {
        if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "ping -n 30 127.0.0.1"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "sh -c 'echo started; sleep 30' & wait"]);
            command
        }
    }

    #[test]
    fn the_program_and_its_children_are_stopped() {
        let mut command = parent_with_a_child();
        command.stdout(Stdio::piped());
        let mut child = own_group(&mut command).spawn().unwrap();
        // Wait for the child's first line: stopping the tree before the child
        // exists would leave nothing to test (on Windows `taskkill /T` could
        // then miss a child started a moment later).
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut first = String::new();
        while first.trim().is_empty() {
            first.clear();
            assert!(
                out.read_line(&mut first).unwrap() > 0,
                "the child never started"
            );
        }
        let start = Instant::now();
        kill_tree(&mut child);
        // The child kept the pipe open; with it stopped, reading ends at once.
        let mut rest = String::new();
        out.read_to_string(&mut rest).unwrap();
        assert!(start.elapsed() < Duration::from_secs(10));
    }
}
