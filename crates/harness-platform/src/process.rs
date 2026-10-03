//! Stopping a program together with everything it started. `npx`, for
//! example, starts the real MCP server as its own child; stopping only `npx`
//! would leave the server running.
//!
//! On Linux and macOS the program gets its own process group, and the whole
//! group is stopped. On Windows `taskkill /T` stops the program and every
//! process it started.

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

/// Stops `child` and everything it started (for a child started with
/// [`own_group`]), then waits for it so no finished process is left behind.
pub fn kill_tree(child: &mut Child) {
    let pid = child.id().to_string();
    #[cfg(unix)]
    let mut killer = {
        let mut killer = Command::new("kill");
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
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A program that starts a long-running child of its own.
    fn parent_with_a_child() -> Command {
        if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "ping -n 30 127.0.0.1 >NUL"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 30 & wait"]);
            command
        }
    }

    #[test]
    fn the_program_and_its_children_are_stopped() {
        let mut command = parent_with_a_child();
        command.stdout(Stdio::piped());
        let mut child = own_group(&mut command).spawn().unwrap();
        let start = Instant::now();
        kill_tree(&mut child);
        // The child kept the pipe open; with it stopped, reading ends at once.
        let mut out = String::new();
        std::io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut out).unwrap();
        assert!(start.elapsed() < Duration::from_secs(10));
    }
}
