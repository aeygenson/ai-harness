//! Lisa's stop buttons, through the whole run loop with the mock agent:
//! «Stop after this step», «Stop now», and what happens after them.

use std::collections::BTreeMap;
use std::fs;
use std::process::Command;

use harness_agents::{MockAgent, MockStep};
use harness_core::git::Repo;
use harness_core::skills::Skills;
use harness_core::task::handoff::{NextStep, Role, Verdict};
use harness_core::task::manifest::AgentFacts;
use harness_core::task::orchestrator::{
    self, continue_stopped, create_task, record_human_decision, RunSetup, StopReason,
};
use harness_core::task::scenario::Workflow;
use harness_core::task::stop_request::StopRequest;
use harness_core::task::store::TaskStore;
use harness_core::task::tripwire::Watched;
use harness_core::task::{Stage, TaskState, WaitReason, DEFAULT_MAX_ROUNDS};
use tempfile::TempDir;

/// A project with one commit and a task whose developer works now.
fn developer_working() -> (TempDir, Repo, TaskStore, TaskState) {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repo::init(dir.path()).unwrap();
    fs::write(dir.path().join("README.md"), "A parser\n").unwrap();
    repo.commit_all("first commit").unwrap();
    let (store, mut state) = create_task(
        &repo,
        "task-001",
        "Build a parser",
        Workflow::Task,
        DEFAULT_MAX_ROUNDS,
    )
    .unwrap();
    state.stage = Stage::Working(Role::Developer);
    (dir, repo, store, state)
}

/// Runs the roles with `stop` as Lisa's stop buttons.
async fn run_with(
    repo: &Repo,
    store: &TaskStore,
    state: &mut TaskState,
    agent: &MockAgent,
    stop: &StopRequest,
) -> StopReason {
    let watched = Watched::for_repo(repo).unwrap();
    let setup = RunSetup {
        skills: &Skills::none(),
        watched: &watched,
        agents: AgentFacts {
            config: None,
            versions: &BTreeMap::new(),
        },
        stop,
    };
    orchestrator::run_watching(repo, store, state, agent, setup)
        .await
        .unwrap()
}

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

fn half_done() -> Vec<(String, String)> {
    vec![
        ("src/half_done.rs".to_string(), "fn half() {}".to_string()),
        (
            "README.md".to_string(),
            "A parser, in progress\n".to_string(),
        ),
    ]
}

fn stopped(role: Role) -> StopReason {
    StopReason::WaitingForHuman(WaitReason::StoppedByLisa(role))
}

/// The developer finishes and asks Lisa, which ends the run.
fn developer_finishes() -> MockStep {
    MockStep::finish_writing(
        Verdict::NeedsHuman,
        NextStep::To(Role::Human),
        &[("src/parser.rs", "fn parse() {}")],
    )
}

#[tokio::test]
async fn stop_after_step_keeps_the_step_and_stops_before_the_next_role() {
    let (_dir, repo, store, mut state) = developer_working();
    let agent = MockAgent::new()
        .then(
            Role::Developer,
            MockStep::finish_writing(
                Verdict::Approved,
                NextStep::To(Role::Tester),
                &[("src/parser.rs", "fn parse() {}")],
            ),
        )
        .then(
            Role::Tester,
            MockStep::finish(Verdict::Approved, NextStep::To(Role::Security)),
        )
        .then(
            Role::Security,
            MockStep::finish(Verdict::Approved, NextStep::Done),
        );
    let stop = StopRequest::new();
    stop.toggle_after_step();

    let reason = run_with(&repo, &store, &mut state, &agent, &stop).await;

    // The developer's step is accepted and committed; the tester never started.
    assert_eq!(reason, stopped(Role::Tester));
    assert_eq!(agent.calls(), [Role::Developer]);
    assert_eq!(
        state.stage,
        Stage::WaitingForHuman(WaitReason::StoppedByLisa(Role::Tester))
    );
    assert_eq!(
        TaskStore::open(&repo.runs_dir(), "task-001").unwrap().1,
        state
    );
    assert_eq!(
        store.history().unwrap().last().unwrap().role,
        Role::Developer
    );
    assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
    let log = git_log(&repo);
    assert_eq!(log[0], "task-001 round 1: stopped by Lisa, tester next");
    assert!(log[1].contains("developer (approved)"), "{log:?}");

    // «Continue»: the tester (the role that was next) goes on.
    assert!(continue_stopped(&repo, &store, &mut state).unwrap());
    let reason = run_with(&repo, &store, &mut state, &agent, &StopRequest::new()).await;
    assert_eq!(reason, StopReason::Done);
    assert_eq!(
        agent.calls(),
        [Role::Developer, Role::Tester, Role::Security]
    );
}

#[tokio::test]
async fn stop_after_step_does_not_change_a_stop_the_step_itself_asks_for() {
    let (_dir, repo, store, mut state) = developer_working();
    let agent = MockAgent::new().then(Role::Developer, developer_finishes());
    let stop = StopRequest::new();
    stop.toggle_after_step();

    let reason = run_with(&repo, &store, &mut state, &agent, &stop).await;

    assert_eq!(
        reason,
        StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Developer))
    );
}

#[tokio::test]
async fn stop_now_keeps_the_unfinished_work_and_waits_for_lisa() {
    let (_dir, repo, store, mut state) = developer_working();
    let agent = MockAgent::new()
        .then(Role::Developer, MockStep::StopNow(half_done()))
        .then(Role::Developer, developer_finishes());

    let reason = run_with(&repo, &store, &mut state, &agent, &StopRequest::new()).await;

    // Stopped once, no second attempt; the work stays uncommitted and
    // recorded; only the harness's records and state.json are committed.
    assert_eq!(reason, stopped(Role::Developer));
    assert_eq!(agent.calls(), [Role::Developer]);
    assert_eq!(
        state.stage,
        Stage::WaitingForHuman(WaitReason::StoppedByLisa(Role::Developer))
    );
    assert_eq!(
        repo.changed_files().unwrap(),
        ["README.md", "src/half_done.rs"]
    );
    assert!(store.dir().join("unfinished.json").exists());
    // Its log is kept, but a stop is not a failed attempt.
    assert!(store
        .dir()
        .join("stopped/round-01-developer-1.log")
        .exists());
    assert_eq!(store.failures().unwrap(), Vec::new());
    let log = git_log(&repo);
    assert_eq!(log[0], "task-001 round 1: stopped by Lisa, developer next");
    assert!(log[1].contains("developer stopped by Lisa"), "{log:?}");

    // Lisa's note to the stopped role: it continues from its work.
    record_human_decision(
        &repo,
        &store,
        &mut state,
        Verdict::Approved,
        NextStep::To(Role::Developer),
        "Keep the old API",
    )
    .unwrap();
    let reason = run_with(&repo, &store, &mut state, &agent, &StopRequest::new()).await;

    assert_eq!(
        reason,
        StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Developer))
    );
    let prompt = &agent.prompts()[1];
    assert!(prompt.contains("- src/half_done.rs"), "{prompt}");
    assert!(prompt.contains("Keep the old API"), "{prompt}");
    assert!(repo.root().join("src/parser.rs").exists());
    assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
    assert!(!store.dir().join("unfinished.json").exists());
}

#[tokio::test]
async fn continue_after_stop_now_resumes_the_recorded_work() {
    let (_dir, repo, store, mut state) = developer_working();
    let agent = MockAgent::new()
        .then(Role::Developer, MockStep::StopNow(half_done()))
        .then(Role::Developer, developer_finishes());
    run_with(&repo, &store, &mut state, &agent, &StopRequest::new()).await;

    assert!(continue_stopped(&repo, &store, &mut state).unwrap());
    run_with(&repo, &store, &mut state, &agent, &StopRequest::new()).await;

    assert!(agent.prompts()[1].contains("- README.md"));
    let readme = fs::read_to_string(repo.root().join("README.md")).unwrap();
    assert_eq!(readme, "A parser, in progress\n");
    assert_eq!(repo.changed_files().unwrap(), Vec::<String>::new());
}

#[tokio::test]
async fn a_note_to_another_role_stops_on_the_stopped_roles_changes() {
    let (_dir, repo, store, mut state) = developer_working();
    let agent = MockAgent::new().then(Role::Developer, MockStep::StopNow(half_done()));
    run_with(&repo, &store, &mut state, &agent, &StopRequest::new()).await;

    record_human_decision(
        &repo,
        &store,
        &mut state,
        Verdict::Approved,
        NextStep::To(Role::Architect),
        "Change the design first",
    )
    .unwrap();
    let reason = run_with(&repo, &store, &mut state, &agent, &StopRequest::new()).await;

    // The architect does not start on the developer's half-done files; they
    // and their record stay for Lisa to commit or remove.
    assert!(
        matches!(reason, StopReason::DirtyWorkingTree(_)),
        "{reason:?}"
    );
    assert_eq!(agent.calls(), [Role::Developer]);
    assert!(repo.root().join("src/half_done.rs").exists());
    assert!(store.dir().join("unfinished.json").exists());
}

#[tokio::test]
async fn stop_now_with_forbidden_changes_leaves_them_for_lisa() {
    let (_dir, repo, store, mut state) = developer_working();
    state.stage = Stage::Working(Role::Security);
    let agent = MockAgent::new().then(
        Role::Security,
        MockStep::StopNow(vec![("src/parser.rs".to_string(), "fn x() {}".to_string())]),
    );

    let reason = run_with(&repo, &store, &mut state, &agent, &StopRequest::new()).await;

    // The same checks as after a time limit: nothing is recorded as work.
    assert!(
        matches!(
            reason,
            StopReason::ForbiddenChanges {
                role: Role::Security,
                ..
            }
        ),
        "{reason:?}"
    );
    assert_eq!(state.stage, Stage::Working(Role::Security));
    assert!(!store.dir().join("unfinished.json").exists());
}

#[tokio::test]
async fn stop_now_before_a_role_starts_runs_no_agent() {
    let (_dir, repo, store, mut state) = developer_working();
    let agent = MockAgent::new().then(Role::Developer, developer_finishes());
    let stop = StopRequest::new();
    stop.stop_now();

    let reason = run_with(&repo, &store, &mut state, &agent, &stop).await;

    assert_eq!(reason, stopped(Role::Developer));
    assert_eq!(agent.calls(), Vec::<Role>::new());
}

#[tokio::test]
async fn a_stopped_task_with_unfinished_work_is_not_finished() {
    let (_dir, repo, store, mut state) = developer_working();
    let agent = MockAgent::new().then(Role::Developer, MockStep::StopNow(half_done()));
    run_with(&repo, &store, &mut state, &agent, &StopRequest::new()).await;
    let before = state.clone();

    let error = record_human_decision(
        &repo,
        &store,
        &mut state,
        Verdict::Approved,
        NextStep::Done,
        "Good enough",
    )
    .unwrap_err();

    assert!(
        matches!(&error, orchestrator::RunError::UnfinishedWork { files } if files.len() == 2),
        "{error}"
    );
    assert!(error.to_string().contains("src/half_done.rs"), "{error}");
    assert_eq!(state, before);
    assert_eq!(store.history().unwrap(), Vec::new());

    // Once Lisa removed the files herself, the task may be finished.
    repo.discard_changes().unwrap();
    record_human_decision(
        &repo,
        &store,
        &mut state,
        Verdict::Approved,
        NextStep::Done,
        "Good enough",
    )
    .unwrap();
    assert_eq!(state.stage, Stage::Done);
}
