//! The user's decision that first restores the code to an earlier step, with
//! the mock agent: a new commit, the history kept, the next role told.

use std::fs;
use std::process::Command;

use harness_agents::{MockAgent, MockStep};
use harness_core::git::Repo;
use harness_core::task::handoff::{NextStep, Role, Verdict};
use harness_core::task::orchestrator::{
    self, create_task, record_human_decision, HumanDecision, RunError,
};
use harness_core::task::scenario::Workflow;
use harness_core::task::store::TaskStore;
use harness_core::task::{Stage, TaskState, WaitReason, DEFAULT_MAX_ROUNDS};
use tempfile::TempDir;

/// Commit messages, newest first.
fn git_log(repo: &Repo) -> Vec<String> {
    let out = Command::new("git")
        .current_dir(repo.root())
        .args(["log", "--format=%s"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(String::from)
        .collect()
}

/// A task whose architect wrote a design and whose developer then wrote
/// code and asked the user: it waits for the user.
async fn task_with_code() -> (TempDir, Repo, TaskStore, TaskState) {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repo::init(dir.path()).unwrap();
    fs::write(dir.path().join("README.md"), "A parser\n").unwrap();
    fs::write(dir.path().join("config.local"), "template").unwrap();
    repo.commit_all("first commit").unwrap();
    let (store, mut state) = create_task(
        &repo,
        "task-001",
        "Build a parser",
        Workflow::Task,
        DEFAULT_MAX_ROUNDS,
    )
    .unwrap();
    let agent = MockAgent::new()
        .then(
            Role::Architect,
            MockStep::finish(Verdict::Approved, NextStep::To(Role::Human)),
        )
        .then(
            Role::Developer,
            MockStep::finish_writing(
                Verdict::NeedsHuman,
                NextStep::To(Role::Human),
                &[
                    ("src/parser.rs", "fn parse() {}"),
                    ("README.md", "A parser, changed\n"),
                ],
            ),
        );
    orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();
    let approve = HumanDecision {
        verdict: Verdict::Approved,
        next: NextStep::To(Role::Developer),
        notes: "Go on.",
        restore_to: None,
    };
    record_human_decision(&repo, &store, &mut state, approve).unwrap();
    orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();
    assert!(dir.path().join("src/parser.rs").is_file());
    (dir, repo, store, state)
}

#[tokio::test]
async fn the_code_goes_back_to_the_step_with_a_new_commit_and_the_role_is_told() {
    let (dir, repo, store, mut state) = task_with_code().await;
    let architect = store.steps().unwrap()[0].dir.clone();

    let decision = HumanDecision {
        verdict: Verdict::Rejected,
        next: NextStep::To(Role::Developer),
        notes: "Start the code again, smaller.",
        restore_to: Some(&architect),
    };
    record_human_decision(&repo, &store, &mut state, decision).unwrap();

    // The code is as after the architect; the history stays.
    assert!(!dir.path().join("src/parser.rs").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("README.md")).unwrap(),
        "A parser\n"
    );
    let log = git_log(&repo);
    assert!(
        log[0].contains("human (rejected) - Start the code again"),
        "{log:?}"
    );
    assert_eq!(
        log[1],
        "task-001 round 1: code restored to round-01/01-architect by the user"
    );
    assert!(log.iter().any(|m| m.contains("developer")), "{log:?}");
    assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
    // The user's note comes first; the next role also learns about the restore.
    let steps = store.steps().unwrap();
    let human = steps.last().unwrap();
    assert!(human.notes.starts_with("Start the code again, smaller."));
    assert!(
        human.notes.contains(
            "restored the code outside .harness/ to how it was after step round-01/01-architect"
        ),
        "{}",
        human.notes
    );
    assert!(steps.iter().any(|s| s.handoff.role == Role::Developer));
}

#[tokio::test]
async fn uncommitted_changes_stop_the_restore_before_anything_changes() {
    let (dir, repo, store, mut state) = task_with_code().await;
    let architect = store.steps().unwrap()[0].dir.clone();
    fs::write(dir.path().join("mine.txt"), "my own work").unwrap();
    let before = git_log(&repo);

    let decision = HumanDecision {
        verdict: Verdict::Rejected,
        next: NextStep::To(Role::Developer),
        notes: "Again.",
        restore_to: Some(&architect),
    };
    let error = record_human_decision(&repo, &store, &mut state, decision).unwrap_err();

    assert!(
        matches!(&error, RunError::NotClean { files } if files == &["mine.txt".to_string()]),
        "{error}"
    );
    assert_eq!(git_log(&repo), before);
    assert!(dir.path().join("src/parser.rs").is_file());
    assert_eq!(
        fs::read_to_string(dir.path().join("mine.txt")).unwrap(),
        "my own work"
    );
}

#[tokio::test]
async fn a_step_outside_the_branch_history_is_refused() {
    let (_dir, repo, store, mut state) = task_with_code().await;
    let nowhere = store.dir().join("round-09/01-architect");

    let decision = HumanDecision {
        verdict: Verdict::Rejected,
        next: NextStep::To(Role::Developer),
        notes: "",
        restore_to: Some(&nowhere),
    };
    let error = record_human_decision(&repo, &store, &mut state, decision).unwrap_err();

    assert!(
        matches!(&error, RunError::NoStepCommit { step } if step == "round-09/01-architect"),
        "{error}"
    );
}

#[tokio::test]
async fn ignored_files_stay_out_of_the_restore_commit() {
    let (dir, repo, store, mut state) = task_with_code().await;
    let architect = store.steps().unwrap()[0].dir.clone();
    // Later the user ignores secret.env; an older .gitignore does not.
    fs::write(dir.path().join(".gitignore"), "secret.env\n").unwrap();
    repo.commit_all("ignore secrets").unwrap();
    fs::write(dir.path().join("secret.env"), "KEY=1").unwrap();

    let decision = HumanDecision {
        verdict: Verdict::Rejected,
        next: NextStep::To(Role::Developer),
        notes: "Again.",
        restore_to: Some(&architect),
    };
    record_human_decision(&repo, &store, &mut state, decision).unwrap();

    assert!(!dir.path().join(".gitignore").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("secret.env")).unwrap(),
        "KEY=1"
    );
    assert_eq!(repo.file_at_head("secret.env"), None);
    assert_eq!(repo.changed_files().unwrap(), ["secret.env".to_string()]);
}

#[tokio::test]
async fn an_ignored_file_the_step_had_in_git_stops_the_restore() {
    let (dir, repo, store, mut state) = task_with_code().await;
    let architect = store.steps().unwrap()[0].dir.clone();
    // config.local was in git at that step; now it is ignored and the user's own.
    fs::write(dir.path().join(".gitignore"), "config.local\n").unwrap();
    Command::new("git")
        .current_dir(dir.path())
        .args(["rm", "-q", "--cached", "config.local"])
        .status()
        .unwrap();
    repo.commit_all("keep config.local local").unwrap();
    fs::write(dir.path().join("config.local"), "MY SETTINGS").unwrap();
    let before = git_log(&repo);

    let decision = HumanDecision {
        verdict: Verdict::Rejected,
        next: NextStep::To(Role::Developer),
        notes: "Again.",
        restore_to: Some(&architect),
    };
    let error = record_human_decision(&repo, &store, &mut state, decision).unwrap_err();

    assert!(
        matches!(&error, RunError::IgnoredInTheWay { files } if files == &["config.local".to_string()]),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("config.local")).unwrap(),
        "MY SETTINGS"
    );
    assert_eq!(git_log(&repo), before);
    assert!(dir.path().join("src/parser.rs").is_file());
}

#[tokio::test]
async fn a_refused_decision_restores_nothing() {
    let (dir, repo, store, mut state) = task_with_code().await;
    let architect = store.steps().unwrap()[0].dir.clone();
    let before = git_log(&repo);

    // A rejection cannot finish the task.
    let decision = HumanDecision {
        verdict: Verdict::Rejected,
        next: NextStep::Done,
        notes: "Stop here.",
        restore_to: Some(&architect),
    };
    let error = record_human_decision(&repo, &store, &mut state, decision).unwrap_err();
    assert!(matches!(error, RunError::Store(_)), "{error}");

    assert_eq!(git_log(&repo), before);
    assert!(dir.path().join("src/parser.rs").is_file());
}

#[tokio::test]
async fn a_project_inside_a_larger_repository_is_refused() {
    let outer = tempfile::tempdir().unwrap();
    let outer_repo = Repo::init(outer.path()).unwrap();
    fs::write(outer.path().join("other.txt"), "outer\n").unwrap();
    let project = outer.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("README.md"), "inner\n").unwrap();
    outer_repo.commit_all("first commit").unwrap();
    let repo = Repo::open(&project).unwrap();
    let (store, mut state) = create_task(
        &repo,
        "task-001",
        "Build a parser",
        Workflow::Task,
        DEFAULT_MAX_ROUNDS,
    )
    .unwrap();
    state.stage = Stage::WaitingForHuman(WaitReason::ApproveDesign);
    let before = git_log(&outer_repo);

    let decision = HumanDecision {
        verdict: Verdict::Rejected,
        next: NextStep::To(Role::Developer),
        notes: "Again.",
        restore_to: Some(&store.dir().join("round-01/01-architect")),
    };
    let error = record_human_decision(&repo, &store, &mut state, decision).unwrap_err();

    assert!(matches!(error, RunError::NotTopLevel), "{error}");
    assert_eq!(git_log(&outer_repo), before);
}
