//! The real `harness run` with a fake `claude` that leaves a helper running:
//! when the terminal window is closed (`SIGHUP`) or the harness is killed
//! (`SIGTERM`), the agent and everything it started must stop too.
//!
//! Sending these signals is how Linux and macOS close a terminal; Windows
//! has no such signal to send from a test, so this file runs only on Unix.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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

/// `harness <args>` in `project`, with `home` as the home folder and `bin` first in PATH.
fn harness(home: &Path, bin: &Path, project: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_harness"));
    command
        .args(args)
        .current_dir(project)
        .env("HOME", home)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@localhost")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@localhost");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![bin.to_path_buf()];
    dirs.extend(std::env::split_paths(&path));
    command.env("PATH", std::env::join_paths(dirs).unwrap());
    command
}

/// Waits up to 20 seconds for `done`.
fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < Duration::from_secs(20), "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Is the process with id `pid` still there? `kill -0` only checks.
fn is_running(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// A project with a task, a saved Claude login and a fake `claude` that
/// starts a long helper, says it is ready, and then works for a long time.
struct Setup {
    _dirs: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    project: PathBuf,
    helper_pid: PathBuf,
    ready: PathBuf,
}

fn setup() -> Setup {
    let dirs = tempfile::tempdir().unwrap();
    let home = dirs.path().join("home");
    let bin = dirs.path().join("bin");
    let project = dirs.path().join("project");
    let helper_pid = dirs.path().join("helper.pid");
    let ready = dirs.path().join("ready");
    fs::create_dir_all(home.join(".harness/credentials/claude")).unwrap();
    fs::write(home.join(".harness/credentials/claude/oauth-token"), "tok").unwrap();
    harness_fake::install(
        &bin,
        "claude",
        &format!(
            // `--version` (asked once when the team is built) only answers.
            "when-arg 1 --version exit 0\nspawn-sleep 60 {}\nsave-text {} yes\nsleep 60\n",
            helper_pid.display(),
            ready.display()
        ),
    );
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("README.md"), "app\n").unwrap();
    git(&project, &["init", "-q"]);
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "first"]);
    for args in [
        &["init"][..],
        &["task", "new", "task-001", "Design a parser"][..],
    ] {
        let out = harness(&home, &bin, &project, args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
    }
    Setup {
        _dirs: dirs,
        home,
        bin,
        project,
        helper_pid,
        ready,
    }
}

/// Starts `harness run` and waits until the fake agent and its helper run.
fn start_run(s: &Setup) -> (Child, String) {
    let child = harness(&s.home, &s.bin, &s.project, &["run", "task-001"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("the fake agent never started", || s.ready.exists());
    let helper = fs::read_to_string(&s.helper_pid).unwrap();
    assert!(is_running(&helper));
    (child, helper)
}

fn signal_stops_the_agent_tree(signal: &str) {
    let s = setup();
    let (mut child, helper) = start_run(&s);

    let status = Command::new("kill")
        .args([&format!("-{signal}"), &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());

    wait_for("the harness did not end", || {
        child.try_wait().unwrap().is_some()
    });
    wait_for("the agent's helper was left running", || {
        !is_running(&helper)
    });
}

#[test]
fn closing_the_terminal_stops_the_agent_with_everything_it_started() {
    signal_stops_the_agent_tree("HUP");
}

#[test]
fn killing_the_harness_stops_the_agent_with_everything_it_started() {
    signal_stops_the_agent_tree("TERM");
}
