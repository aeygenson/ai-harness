//! The real `harness run` in a project on GitHub: the task gets its own
//! folder next to the project, the agent works there, and the project
//! folder stays on its branch.

use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// `git` in `dir`, with a name and email so commits work on any machine.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@localhost"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// `harness <args>` in `project`, with the test's home folder and `bin` first in PATH.
fn harness(project: &Path, home: &Path, bin: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_harness"));
    command
        .args(args)
        .current_dir(project)
        .env("HOME", home)
        .env("USERPROFILE", home)
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

/// Stops the running harness when the test ends, also when it fails halfway,
/// so no fake agent is left running.
struct StopOnDrop(Child);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn the_agent_works_in_the_task_folder_and_the_project_stays_on_its_branch() {
    let dirs = tempfile::tempdir().unwrap();
    let home = dirs.path().join("home");
    let bin = dirs.path().join("bin");
    let project = dirs.path().join("app");
    fs::create_dir_all(home.join(".harness/credentials/claude")).unwrap();
    fs::write(home.join(".harness/credentials/claude/oauth-token"), "tok").unwrap();
    // The fake agent leaves a file in the folder it was started in, then works on.
    harness_fake::install(
        &bin,
        "claude",
        "when-arg 1 --version exit 0\nsave-text started-here yes\nsleep 60\n",
    );
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("README.md"), "app\n").unwrap();
    git(&project, &["init", "-q"]);
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "first"]);
    let run = |args: &[&str]| {
        let out = harness(&project, &home, &bin, args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    run(&["init"]);
    let settings = project.join(".harness/harness.toml");
    let text = fs::read_to_string(&settings).unwrap();
    fs::write(
        &settings,
        format!("{text}\n[github]\nrepo = \"lisa/app\"\n"),
    )
    .unwrap();
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "on GitHub"]);
    let branch = git(&project, &["branch", "--show-current"]);
    run(&["task", "new", "task-001", "Design a parser"]);

    let mut running = StopOnDrop(
        harness(&project, &home, &bin, &["run", "task-001"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let task_folder = dirs.path().join("app.tasks/task-001");
    let start = Instant::now();
    while !task_folder.join("started-here").exists() {
        assert!(
            start.elapsed() <= Duration::from_secs(30),
            "the fake agent never started in {}",
            task_folder.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    run(&["stop", "task-001", "--now"]);
    running.0.wait().unwrap();

    assert!(!project.join("started-here").exists());
    assert_eq!(git(&project, &["branch", "--show-current"]), branch);
    assert!(!project.join(".harness/runs/task-001").exists());
    assert!(task_folder
        .join(".harness/runs/task-001/state.json")
        .is_file());
}
