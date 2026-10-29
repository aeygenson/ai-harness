//! The real `harness run` with a fake `claude` that works for a long time:
//! while it runs, a second `harness` in the same project is refused, and
//! `harness stop` from another terminal stops the run like the TUI's button.
//! The same through the MCP server: an agent creates, starts and stops a task.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// `git` in `dir`, with a name and email so commits work on any machine.
fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@localhost"])
        .args(args)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// A project with a task, a saved Claude login and a fake `claude` that
/// says it is ready and then works for a long time.
struct Setup {
    _dirs: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    project: PathBuf,
    ready: PathBuf,
}

impl Setup {
    fn new() -> Self {
        let dirs = tempfile::tempdir().unwrap();
        let home = dirs.path().join("home");
        let bin = dirs.path().join("bin");
        let project = dirs.path().join("project");
        let ready = dirs.path().join("ready");
        fs::create_dir_all(home.join(".harness/credentials/claude")).unwrap();
        fs::write(home.join(".harness/credentials/claude/oauth-token"), "tok").unwrap();
        harness_fake::install(
            &bin,
            "claude",
            // `--version` (asked once when the team is built) only answers.
            &format!(
                "when-arg 1 --version exit 0\nsave-text {} yes\nsleep 60\n",
                ready.display()
            ),
        );
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.md"), "app\n").unwrap();
        git(&project, &["init", "-q"]);
        git(&project, &["add", "."]);
        git(&project, &["commit", "-q", "-m", "first"]);
        let setup = Self {
            _dirs: dirs,
            home,
            bin,
            project,
            ready,
        };
        for args in [
            &["init"][..],
            &["task", "new", "task-001", "Design a parser"][..],
        ] {
            let out = setup.harness(args).output().unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        }
        setup
    }

    /// `harness <args>` in the project, with the test's home folder and
    /// the fake agent first in PATH.
    fn harness(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_harness"));
        command
            .args(args)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@localhost")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@localhost");
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![self.bin.clone()];
        dirs.extend(std::env::split_paths(&path));
        command.env("PATH", std::env::join_paths(dirs).unwrap());
        command
    }

    /// Starts `harness run task-001` and waits until the fake agent works.
    fn start_run(&self) -> Child {
        let child = self
            .harness(&["run", "task-001"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_for("the fake agent never started", || self.ready.exists());
        child
    }

    fn state(&self) -> String {
        fs::read_to_string(self.project.join(".harness/runs/task-001/state.json")).unwrap()
    }
}

/// A running `harness run` that is killed if the test fails before it ends,
/// so a failed test leaves no harness and fake agent behind.
struct RunGuard(Child);

impl Drop for RunGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits up to 30 seconds for `done`.
fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < Duration::from_secs(30), "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_second_harness_is_refused_and_harness_stop_ends_the_run() {
    let setup = Setup::new();
    let mut run = RunGuard(setup.start_run());

    let second = setup
        .harness(&["approve", "task-001", "--to", "developer"])
        .output()
        .unwrap();
    assert!(!second.status.success(), "{second:?}");
    assert!(
        stderr(&second).contains("another harness is running task-001"),
        "{second:?}"
    );

    let stop = setup
        .harness(&["stop", "task-001", "--now"])
        .output()
        .unwrap();
    assert!(stop.status.success(), "{stop:?}");

    wait_for("the run did not stop", || {
        run.0.try_wait().unwrap().is_some()
    });
    assert!(
        setup.state().contains("stopped_by_lisa"),
        "{}",
        setup.state()
    );
    let again = setup.harness(&["stop", "task-001"]).output().unwrap();
    assert!(!again.status.success(), "nothing runs any more: {again:?}");
    assert!(stderr(&again).contains("no harness is running task-001"));
}

/// A conversation with `harness mcp-serve`, one JSON message per line.
struct Mcp {
    server: RunGuard,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u32,
}

impl Mcp {
    fn start(setup: &Setup) -> Self {
        let mut child = setup
            .harness(&["mcp-serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            server: RunGuard(child),
            input,
            output,
            next_id: 1,
        }
    }

    /// Calls the tool `name` with `arguments` (JSON text) and returns the answer line.
    fn call(&mut self, name: &str, arguments: &str) -> String {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(
            self.input,
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
        )
        .unwrap();
        self.input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        line
    }
}

#[test]
fn an_agent_creates_starts_and_stops_a_task_through_mcp() {
    let setup = Setup::new();
    let mut mcp = Mcp::start(&setup);

    let created = mcp.call("create_task", r#"{"description":"Write a CSV export"}"#);
    assert!(created.contains("Created task-002."), "{created}");

    let started = mcp.call("start_run", r#"{"task":"task-002"}"#);
    assert!(started.contains("task-002 is running"), "{started}");
    wait_for("the fake agent never started", || setup.ready.exists());

    let stopped = mcp.call("stop_run", r#"{"task":"task-002","now":true}"#);
    assert!(stopped.contains("Asked task-002 to stop now"), "{stopped}");
    let state = setup.project.join(".harness/runs/task-002/state.json");
    wait_for("the run did not stop", || {
        fs::read_to_string(&state).is_ok_and(|text| text.contains("stopped_by_lisa"))
    });
    // The stopped run has ended and let go of the project.
    wait_for("the run kept the project locked", || {
        mcp.call("read_run_log", r#"{"task":"task-002"}"#)
            .contains("You stopped task-002")
    });
    drop(mcp.server);
}
