//! The real `harness run` with a fake `claude` that works for a long time:
//! while it runs, a second `harness` in the same project is refused, and
//! `harness stop` from another terminal stops the run like the TUI's button.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
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
    let mut run = setup.start_run();

    let second = setup
        .harness(&["approve", "task-001", "--to", "developer"])
        .output()
        .unwrap();
    assert!(!second.status.success(), "{second:?}");
    assert!(
        stderr(&second).contains("another harness is working on task-001"),
        "{second:?}"
    );

    let stop = setup
        .harness(&["stop", "task-001", "--now"])
        .output()
        .unwrap();
    assert!(stop.status.success(), "{stop:?}");

    wait_for("the run did not stop", || run.try_wait().unwrap().is_some());
    assert!(
        setup.state().contains("stopped_by_lisa"),
        "{}",
        setup.state()
    );
    let again = setup.harness(&["stop", "task-001"]).output().unwrap();
    assert!(!again.status.success(), "nothing runs any more: {again:?}");
    assert!(stderr(&again).contains("no harness is running task-001"));
}
