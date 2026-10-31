//! Stopping a program together with everything it started. `npx`, for
//! example, starts the real MCP server as its own child; stopping only `npx`
//! would leave the server running.
//!
//! On Linux and macOS the program gets its own process group, and the whole
//! group is stopped. On Windows `taskkill /T` stops the program and every
//! process it started. [`detached_command`] is the opposite: a program that keeps
//! running after its starter has ended.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Prepares `command` so that [`kill_tree`] can stop everything it starts.
pub fn own_group(command: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

/// Windows: the started program gets a console without a window. Not
/// `DETACHED_PROCESS` (no console at all): then every console program it
/// starts (git, the agents) would open a window of its own.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Windows: the started program does not get the Ctrl+C sent to the starter.
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// A command for `program` that keeps running after its starter has ended.
///
/// Like `nohup` in a shell: its own process group (Linux, macOS) or a hidden
/// console of its own (Windows), and no input. Add the arguments and send the
/// output to a file; the starter's output may be gone. `why` names the run
/// (for example "harness run task-001").
///
/// On a Linux desktop run by systemd (KDE puts every window in a systemd
/// scope), closing a terminal window stops every process of its scope, own
/// group or not. There the program starts in a scope of its own
/// (`systemd-run --user --scope`) and, where the system allows it, keeps the
/// computer from falling asleep while it runs (`systemd-inhibit`). Each is
/// first tried with `true`; what does not work is left out. The process id
/// of the started child may then be the inhibitor's, not the program's: stop
/// the run through its stop file, not by that id. Elsewhere (macOS, Windows,
/// a login over SSH, a container) the program starts as it is. On Linux and
/// macOS it stays in the starter's session (a new session needs `unsafe`
/// code here).
pub fn detached_command(program: &Path, why: &str) -> Command {
    let mut command = match systemd_wrappers() {
        Some(wrappers) => in_own_scope(&wrappers, program, why),
        None => Command::new(program),
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    command.stdin(Stdio::null());
    command
}

/// The systemd programs that start a run in a scope of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SystemdWrappers {
    /// `systemd-run`.
    run: PathBuf,
    /// `systemd-inhibit`, when this login may keep the computer awake.
    inhibit: Option<PathBuf>,
}

/// The systemd programs that work here; `None` when a scope of its own does
/// not work or is not needed: not Linux, or this program does not run under
/// the user's systemd (an SSH login, a text console, a container), where
/// its own process group already outlives the window and a scope under the
/// user's systemd could end with the last login instead.
fn systemd_wrappers() -> Option<SystemdWrappers> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    if !under_user_systemd(&cgroup) {
        return None;
    }
    let run = crate::program::find("systemd-run")?;
    let inhibit = crate::program::find("systemd-inhibit");
    first_that_works(run, inhibit, works)
}

/// The scope with the sleep lock if `works` says it starts, else the scope
/// alone (the system may refuse the lock), else `None`.
fn first_that_works(
    run: PathBuf,
    inhibit: Option<PathBuf>,
    works: impl Fn(&SystemdWrappers) -> bool,
) -> Option<SystemdWrappers> {
    let with_inhibit = SystemdWrappers {
        run: run.clone(),
        inhibit,
    };
    if with_inhibit.inhibit.is_some() && works(&with_inhibit) {
        return Some(with_inhibit);
    }
    let scope_only = SystemdWrappers { run, inhibit: None };
    works(&scope_only).then_some(scope_only)
}

/// Whether this program runs under the user's own systemd (as every app of
/// a KDE or GNOME desktop does), from the text of `/proc/self/cgroup`, for
/// example `0::/user.slice/user-1000.slice/user@1000.service/app.slice/...`.
fn under_user_systemd(cgroup: &str) -> bool {
    cgroup
        .lines()
        .filter_map(|line| line.strip_prefix("0::"))
        .any(|path| {
            path.split('/')
                .any(|part| part.starts_with("user@") && part.ends_with(".service"))
        })
}

/// How long a check with `true` may take: a few milliseconds when systemd
/// answers; a broken user bus would otherwise make `start_run` wait for
/// D-Bus's own 25-second limit.
const CHECK_LIMIT: Duration = Duration::from_secs(2);

/// Starts `true` the way a run would start, and says whether that worked
/// within [`CHECK_LIMIT`].
fn works(wrappers: &SystemdWrappers) -> bool {
    let started = in_own_scope(wrappers, Path::new("true"), "harness check")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut check) = started else {
        return false;
    };
    let deadline = Instant::now() + CHECK_LIMIT;
    while Instant::now() < deadline {
        match check.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    let _ = check.kill();
    let _ = check.wait();
    false
}

/// `systemd-run --user --scope -- [systemd-inhibit ...] <program>`. The scope
/// starts what follows itself (`exec`), keeping the environment, folder and
/// output. `systemd-inhibit` starts the program as its child (so the process
/// id the starter knows is the inhibitor's) and ends with its exit code.
/// Neither asks for a password: the user may be away.
fn in_own_scope(wrappers: &SystemdWrappers, program: &Path, why: &str) -> Command {
    let mut command = Command::new(&wrappers.run);
    command
        // `--quiet`: no "Running scope as unit" line in the run's log;
        // `--collect`: a scope that ended badly is not kept as "failed".
        .args([
            "--user",
            "--scope",
            "--quiet",
            "--collect",
            "--no-ask-password",
        ])
        .arg(format!("--description={why}"))
        .arg("--");
    if let Some(inhibit) = &wrappers.inhibit {
        command
            .arg(inhibit)
            // Only "sleep": "idle" would also keep the screen from locking
            // (KDE), and a shutdown should still end the run.
            .args([
                "--what=sleep",
                "--who=harness",
                "--mode=block",
                "--no-ask-password",
            ])
            .arg(format!("--why={why}"));
    }
    command.arg(program);
    command
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

    fn wrappers(inhibit: bool) -> SystemdWrappers {
        SystemdWrappers {
            run: PathBuf::from("/usr/bin/systemd-run"),
            inhibit: inhibit.then(|| PathBuf::from("/usr/bin/systemd-inhibit")),
        }
    }

    fn args_of(command: &Command) -> Vec<&std::ffi::OsStr> {
        command.get_args().collect()
    }

    const SCOPE: [&str; 7] = [
        "--user",
        "--scope",
        "--quiet",
        "--collect",
        "--no-ask-password",
        "--description=harness run task-001",
        "--",
    ];

    #[test]
    fn a_run_in_its_own_scope_keeps_the_computer_awake_and_runs_the_program_last() {
        let mut command = in_own_scope(
            &wrappers(true),
            Path::new("/bin/harness"),
            "harness run task-001",
        );
        command.args(["run", "task-001"]);

        assert_eq!(command.get_program(), "/usr/bin/systemd-run");
        let mut expected = SCOPE.to_vec();
        expected.extend([
            "/usr/bin/systemd-inhibit",
            "--what=sleep",
            "--who=harness",
            "--mode=block",
            "--no-ask-password",
            "--why=harness run task-001",
            "/bin/harness",
            "run",
            "task-001",
        ]);
        assert_eq!(args_of(&command), expected);
    }

    #[test]
    fn without_the_sleep_lock_the_scope_runs_the_program_itself() {
        let command = in_own_scope(
            &wrappers(false),
            Path::new("/bin/harness"),
            "harness run task-001",
        );

        let mut expected = SCOPE.to_vec();
        expected.push("/bin/harness");
        assert_eq!(args_of(&command), expected);
    }

    #[test]
    fn only_a_program_under_the_user_s_systemd_needs_a_scope() {
        let konsole = concat!(
            "0::/user.slice/user-1000.slice/user@1000.service",
            "/app.slice/app-org.kde.konsole-1.scope\n"
        );
        let ssh = "0::/user.slice/user-1000.slice/session-4.scope\n";
        let container = "0::/\n";
        let cgroup_v1 = "1:name=systemd:/user.slice/user-1000.slice/user@1000.service/app.slice\n";

        assert!(under_user_systemd(konsole));
        assert!(!under_user_systemd(ssh));
        assert!(!under_user_systemd(container));
        assert!(!under_user_systemd(cgroup_v1));
    }

    #[test]
    fn a_wrapper_that_does_not_start_is_left_out() {
        let run = || PathBuf::from("/usr/bin/systemd-run");
        let inhibit = || Some(PathBuf::from("/usr/bin/systemd-inhibit"));

        let both = first_that_works(run(), inhibit(), |_| true);
        let lock_refused = first_that_works(run(), inhibit(), |w| w.inhibit.is_none());
        let no_inhibit_program = first_that_works(run(), None, |_| true);
        let nothing_works = first_that_works(run(), inhibit(), |_| false);

        assert_eq!(both, Some(wrappers(true)));
        assert_eq!(lock_refused, Some(wrappers(false)));
        assert_eq!(no_inhibit_program, Some(wrappers(false)));
        assert_eq!(nothing_works, None);
    }

    #[test]
    fn a_check_that_fails_says_so() {
        let missing = SystemdWrappers {
            run: PathBuf::from("/no/such/systemd-run"),
            inhibit: None,
        };

        assert!(!works(&missing));
    }
}
