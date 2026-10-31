//! The server's tools that change something: create a task, start its run in
//! the background, stop it, and read what the background run printed.
//!
//! They do what `harness task new`, `harness run` and `harness stop` do, with
//! the same code and the same run lock (`harness_core::task::run_lock`), so a
//! task started here and one started in the TUI or a terminal never work in
//! one folder at once. Design: `docs/design.md`, section 5.8.

use std::fs::{self, OpenOptions};
use std::path::Path;
use std::process::Child;
use std::thread;
use std::time::{Duration, Instant};

use harness_core::config::Config;
use harness_core::git::{Repo, HARNESS_DIR};
use harness_core::task::branch;
use harness_core::task::orchestrator::{self, SameText};
use harness_core::task::run_lock::{self, RunLock};
use harness_core::task::stop_request::StopWhen;
use harness_core::task::store;
use harness_core::text::safe;
use harness_platform::{process, program};

use super::read::{tail, LOG_TAIL_CHARS};

/// How long `start_run` watches the new run before it answers: long enough
/// to catch a run that fails at once (a missing login, a busy project),
/// short enough for an agent waiting on the answer.
const START_WATCH: Duration = Duration::from_secs(3);

/// How often `start_run` looks whether the new run has already ended.
const START_POLL: Duration = Duration::from_millis(100);

/// The longest task description `create_task` takes: a task is a few
/// paragraphs, and the text is committed and shown to every role.
const MAX_DESCRIPTION_CHARS: usize = 20_000;

/// `create_task`: a new task with `description`, id `task` or the next free
/// one, made and committed like `harness task new`.
pub(super) fn create_task(
    root: &Path,
    task: Option<&str>,
    description: &str,
    allow_duplicate: bool,
) -> Result<String, String> {
    if description.trim().is_empty() {
        return Err("the description is empty: say what the task should do".into());
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(format!(
            "the description is longer than {MAX_DESCRIPTION_CHARS} characters; \
             put the details in a file of the project and refer to it"
        ));
    }
    let repo = Repo::open(root).map_err(|e| e.to_string())?;
    let mut lock = RunLock::hold(&repo).map_err(|e| e.to_string())?;
    let task = match task {
        Some(task) => task.to_string(),
        None => branch::next_task_id(&repo).map_err(|e| e.to_string())?,
    };
    lock.set_task(&task).map_err(|e| e.to_string())?;
    let config = Config::load(&root.join(HARNESS_DIR)).map_err(|e| e.to_string())?;
    let same_text = if allow_duplicate {
        SameText::Allow
    } else {
        SameText::Refuse
    };
    orchestrator::create_project_task(&repo, &config, &task, description, same_text)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Created {task}. Start its roles with start_run; the architect writes the design first."
    ))
}

/// `start_run`: starts `harness run <task>` with the program `harness` as a
/// program of its own, which goes on when this server ends, and answers once
/// it is clearly running or has already ended.
pub(super) fn start_run(root: &Path, harness: &Path, task: &str) -> Result<String, String> {
    let repo = Repo::open(root).map_err(|e| e.to_string())?;
    // First, because it also checks that the id is a task id.
    let log_path = run_lock::background_log(&repo, task).map_err(|e| e.to_string())?;
    check_task_exists(&repo, task)?;
    // Only a check that the project is free: the run takes the lock itself,
    // and two holders would refuse each other. Dropped before the run starts.
    drop(RunLock::hold(&repo).map_err(|e| e.to_string())?);
    let child = spawn_run(root, harness, task, &log_path)?;
    let ended = watch_start(child);
    let log = read_tail(&log_path);
    match ended {
        None => Ok(format!(
            "{task} is running (it may still fail while starting: a missing login, \
             for example). Follow it with wait_for_change and task_status; stop it \
             with stop_run. What the harness prints is in read_run_log."
        )),
        Some(true) => Ok(format!("The run of {task} has already ended:\n{log}")),
        Some(false) => Err(format!("The run of {task} failed:\n{log}")),
    }
}

/// `stop_run`: asks the harness running `task` to stop, at once with `now`
/// or once the running role has finished, like `harness stop`.
pub(super) fn stop_run(root: &Path, task: &str, now: bool) -> Result<String, String> {
    let repo = Repo::open(root).map_err(|e| e.to_string())?;
    let when = if now {
        StopWhen::Now
    } else {
        StopWhen::AfterStep
    };
    run_lock::ask_to_stop(&repo, task, when).map_err(|e| e.to_string())?;
    Ok(if now {
        format!("Asked {task} to stop now. Continue it later with start_run.")
    } else {
        format!(
            "Asked {task} to stop once the running role has finished. \
             Continue it later with start_run."
        )
    })
}

/// `read_run_log`: the end of what the last background run of `task` printed.
pub(super) fn read_run_log(root: &Path, task: &str) -> Result<String, String> {
    let repo = Repo::open(root).map_err(|e| e.to_string())?;
    let path = run_lock::background_log(&repo, task).map_err(|e| e.to_string())?;
    if !is_plain_file(&path) {
        return Err(format!("{task} has not been started with start_run yet"));
    }
    Ok(read_tail(&path))
}

/// Refuses a task that is neither in the folder nor on a branch of its own.
fn check_task_exists(repo: &Repo, task: &str) -> Result<(), String> {
    let ids = store::task_ids(&repo.runs_dir()).map_err(|e| e.to_string())?;
    let on_branch = repo
        .has_branch(&branch::task_branch(task))
        .map_err(|e| e.to_string())?;
    if ids.iter().any(|id| id == task) || on_branch {
        Ok(())
    } else {
        Err(format!("there is no task {task}; see list_tasks"))
    }
}

/// Starts `harness -C <root> run <task>` detached, its output going to `log`.
fn spawn_run(root: &Path, harness: &Path, task: &str, log: &Path) -> Result<Child, String> {
    let cannot = |e: std::io::Error| format!("cannot start the run of {task}: {e}");
    if let Some(folder) = log.parent() {
        fs::create_dir_all(folder).map_err(cannot)?;
    }
    // A fresh file: removing the old name first and `create_new` never write
    // through a link (or into a hard-linked file) put in its place.
    match fs::remove_file(log) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(cannot(e)),
        _ => {}
    }
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(log)
        .map_err(cannot)?;
    let errors = output.try_clone().map_err(cannot)?;
    // After `harness update` this program's own file may be gone (Linux
    // then names it "... (deleted)"): the new one is found in PATH.
    let harness = if harness.is_file() {
        harness.to_path_buf()
    } else {
        program::resolve("harness")
    };
    let mut command = process::detached_command(&harness, &format!("harness run {task}"));
    command
        .arg("-C")
        .arg(root)
        .args(["run", task])
        .stdout(output)
        .stderr(errors);
    command.spawn().map_err(cannot)
}

/// Watches a new run for [`START_WATCH`]: `None` while it still runs (then
/// a thread waits for it, so no finished process is left behind), or
/// whether it ended well.
fn watch_start(mut child: Child) -> Option<bool> {
    let deadline = Instant::now() + START_WATCH;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.success()),
            Ok(None) => thread::sleep(START_POLL),
            // It cannot be watched; it runs on, and its log tells the rest.
            Err(_) => break,
        }
    }
    thread::spawn(move || child.wait());
    None
}

/// The end of the log at `path`, without terminal codes. Only a plain file
/// is read: a link an agent put there could point at a private file.
fn read_tail(path: &Path) -> String {
    if !is_plain_file(path) {
        return String::new();
    }
    let text = fs::read(path).unwrap_or_default();
    tail(&safe(&String::from_utf8_lossy(&text)), LOG_TAIL_CHARS).0
}

/// Is `path` a plain file (not a link or folder)?
fn is_plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A git project with `.harness/harness.toml` committed.
    fn project() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        let harness_dir = dir.path().join(HARNESS_DIR);
        fs::create_dir_all(&harness_dir).unwrap();
        fs::write(harness_dir.join("harness.toml"), "max_rounds = 5\n").unwrap();
        repo.commit_all("settings").unwrap();
        (dir, repo)
    }

    #[test]
    fn create_task_makes_the_next_task_and_refuses_the_same_text() {
        let (dir, repo) = project();

        let answer = create_task(dir.path(), None, "Build a parser", false).unwrap();

        assert!(answer.starts_with("Created task-001."), "{answer}");
        assert!(repo.runs_dir().join("task-001/task.md").is_file());
        let again = create_task(dir.path(), None, "Build  a parser", false).unwrap_err();
        assert!(again.contains("task-001"), "{again}");
        create_task(dir.path(), Some("task-007"), "Build a parser", true).unwrap();
        assert!(repo.runs_dir().join("task-007").is_dir());
    }

    #[test]
    fn create_task_refuses_while_another_harness_works() {
        let (dir, repo) = project();
        let _other = RunLock::hold(&repo).unwrap();

        let refused = create_task(dir.path(), None, "Build a parser", false).unwrap_err();

        assert!(refused.contains("another harness is working"), "{refused}");
        assert!(!repo.runs_dir().join("task-001").exists());
    }

    #[test]
    fn start_run_starts_harness_run_and_reports_how_it_ended() {
        let (dir, _repo) = project();
        create_task(dir.path(), None, "Build a parser", false).unwrap();
        let bin = dir.path().join("bin");
        let args = dir.path().join("args.txt");
        let harness = harness_fake::install(
            &bin,
            "harness",
            &format!("save-args {}\nprint Waiting for you\n", args.display()),
        );

        let answer = start_run(dir.path(), &harness, "task-001").unwrap();

        assert!(answer.contains("has already ended"), "{answer}");
        assert!(answer.contains("Waiting for you"), "{answer}");
        let saved = fs::read_to_string(&args).unwrap();
        assert!(saved.ends_with("run\ntask-001\n"), "{saved}");
        let log = read_run_log(dir.path(), "task-001").unwrap();
        assert!(log.contains("Waiting for you"), "{log}");
    }

    #[test]
    fn a_run_that_fails_at_once_is_an_error_with_its_output() {
        let (dir, _repo) = project();
        create_task(dir.path(), None, "Build a parser", false).unwrap();
        let harness = harness_fake::install(
            &dir.path().join("bin"),
            "harness",
            "eprint no Claude token saved\nexit 1\n",
        );

        let failed = start_run(dir.path(), &harness, "task-001").unwrap_err();

        assert!(failed.contains("no Claude token saved"), "{failed}");
    }

    #[test]
    fn start_run_refuses_an_unknown_task_and_a_busy_project() {
        let (dir, repo) = project();
        let harness = dir.path().join("no-harness-needed");
        let unknown = start_run(dir.path(), &harness, "task-001").unwrap_err();
        assert!(unknown.contains("there is no task task-001"), "{unknown}");

        create_task(dir.path(), None, "Build a parser", false).unwrap();
        let _other = RunLock::hold(&repo).unwrap();
        let busy = start_run(dir.path(), &harness, "task-001").unwrap_err();
        assert!(busy.contains("another harness is working"), "{busy}");
    }

    #[cfg(unix)]
    #[test]
    fn the_log_is_never_written_or_read_through_a_link() {
        let (dir, repo) = project();
        create_task(dir.path(), None, "Build a parser", false).unwrap();
        let private = dir.path().join("private");
        fs::write(&private, "secret").unwrap();
        let log = run_lock::background_log(&repo, "task-001").unwrap();
        fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&private, &log).unwrap();

        let unread = read_run_log(dir.path(), "task-001").unwrap_err();
        assert!(!unread.contains("secret"), "{unread}");
        let harness = harness_fake::install(&dir.path().join("bin"), "harness", "print ran\n");
        start_run(dir.path(), &harness, "task-001").unwrap();

        assert_eq!(fs::read_to_string(&private).unwrap(), "secret");
        assert!(read_run_log(dir.path(), "task-001")
            .unwrap()
            .contains("ran"));
    }

    #[test]
    fn a_bad_task_id_or_a_huge_description_is_refused_first() {
        let (dir, _repo) = project();
        let harness = dir.path().join("no-harness-needed");
        let bad = start_run(dir.path(), &harness, "../x").unwrap_err();
        assert!(bad.contains("../x") || bad.contains("task id"), "{bad}");
        let huge = "x".repeat(MAX_DESCRIPTION_CHARS + 1);
        let refused = create_task(dir.path(), None, &huge, false).unwrap_err();
        assert!(refused.contains("longer than"), "{refused}");
    }

    #[test]
    fn stop_run_says_when_nothing_runs() {
        let (dir, _repo) = project();
        let refused = stop_run(dir.path(), "task-001", true).unwrap_err();
        assert!(
            refused.contains("no harness is running task-001"),
            "{refused}"
        );
        let unknown = read_run_log(dir.path(), "task-001").unwrap_err();
        assert!(unknown.contains("not been started"), "{unknown}");
    }
}
