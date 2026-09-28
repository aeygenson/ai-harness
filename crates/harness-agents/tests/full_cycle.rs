//! Runs the whole loop of four roles with the mock agent.
//! Files in `tests/` are integration tests: they use the crates from outside,
//! exactly like the CLI will.

use std::fs;
use std::process::Command;

use harness_agents::{MockAgent, MockStep};
use harness_core::git::Repo;
use harness_core::handoff::{NextStep, Role, Verdict};
use harness_core::orchestrator::{
    self, create_task, record_human_decision, StopReason, ATTEMPTS_PER_ROLE,
};
use harness_core::store::TaskStore;
use harness_core::task::{Stage, TaskState, WaitReason, DEFAULT_MAX_ROUNDS};
use tempfile::TempDir;

fn approve(next: Role) -> MockStep {
    MockStep::finish(Verdict::Approved, NextStep::To(next))
}

/// A fresh project folder with git and one commit. Keep the `TempDir`
/// alive: when it is dropped, the folder is deleted.
fn new_project() -> (TempDir, Repo) {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repo::init(dir.path()).unwrap();
    fs::write(dir.path().join("README.md"), "A parser\n").unwrap();
    repo.commit_all("first commit").unwrap();
    (dir, repo)
}

fn new_task(repo: &Repo) -> (TaskStore, TaskState) {
    create_task(repo, "task-001", "Build a parser", DEFAULT_MAX_ROUNDS).unwrap()
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

#[tokio::test]
async fn happy_path_with_lisa_approving_the_design() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    let agent = MockAgent::new()
        .then(Role::Architect, approve(Role::Human))
        .then(Role::Developer, approve(Role::Tester))
        .then(Role::Tester, approve(Role::Security))
        .then(
            Role::Security,
            MockStep::finish(Verdict::Approved, NextStep::Done),
        );

    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();
    assert_eq!(stop, StopReason::WaitingForHuman(WaitReason::ApproveDesign));

    record_human_decision(
        &repo,
        &store,
        &mut state,
        Verdict::Approved,
        NextStep::To(Role::Developer),
        "Looks good.",
    )
    .unwrap();

    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();
    assert_eq!(stop, StopReason::Done);
    assert_eq!(
        agent.calls(),
        [
            Role::Architect,
            Role::Developer,
            Role::Tester,
            Role::Security
        ]
    );
    let roles: Vec<Role> = store.history().unwrap().iter().map(|h| h.role).collect();
    assert_eq!(
        roles,
        [
            Role::Architect,
            Role::Human,
            Role::Developer,
            Role::Tester,
            Role::Security
        ]
    );
    assert_eq!(
        git_log(&repo),
        [
            "task-001 round 1: security (approved) - Approved",
            "task-001 round 1: tester (approved) - Approved",
            "task-001 round 1: developer (approved) - Approved",
            "task-001 round 1: human (approved) - Looks good.",
            "task-001 round 1: architect (approved) - Approved",
            "task-001: new task",
            "harness: ignore scratch folders",
            "first commit",
        ]
    );
    assert!(repo.changed_files().unwrap().is_empty());
}

#[tokio::test]
async fn tester_sends_a_bug_back_and_the_developer_fixes_it() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    state.stage = Stage::Working(Role::Developer);
    let agent = MockAgent::new()
        .then(Role::Developer, approve(Role::Tester))
        .then(
            Role::Tester,
            MockStep::finish(Verdict::Rejected, NextStep::To(Role::Developer)),
        )
        .then(Role::Developer, approve(Role::Tester))
        .then(Role::Tester, approve(Role::Security))
        .then(
            Role::Security,
            MockStep::finish(Verdict::Approved, NextStep::Done),
        );

    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(stop, StopReason::Done);
    assert_eq!(state.round, 2);
    assert!(store.dir().join("round-01/02-tester").exists());
    assert!(store.dir().join("round-02/01-developer").exists());
}

#[tokio::test]
async fn a_bad_first_attempt_is_retried() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    let agent = MockAgent::new()
        .then(Role::Architect, MockStep::WriteGarbage)
        .then(Role::Architect, approve(Role::Human));

    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(stop, StopReason::WaitingForHuman(WaitReason::ApproveDesign));
    assert_eq!(agent.calls(), [Role::Architect, Role::Architect]);
}

#[tokio::test]
async fn a_role_that_keeps_failing_stops_the_task() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    let before = state.clone();
    let agent = MockAgent::new(); // no script: every run writes nothing

    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert!(matches!(
        stop,
        StopReason::RoleFailed {
            role: Role::Architect,
            ..
        }
    ));
    assert_eq!(agent.calls().len(), ATTEMPTS_PER_ROLE as usize);
    assert_eq!(state, before);
    assert!(store.history().unwrap().is_empty());
}

#[tokio::test]
async fn a_forbidden_route_is_refused_and_explained() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    state.stage = Stage::Working(Role::Developer);
    // The developer tries to skip the tester, twice.
    let agent = MockAgent::new()
        .then(Role::Developer, approve(Role::Security))
        .then(Role::Developer, approve(Role::Security));

    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    match stop {
        StopReason::RoleFailed { role, problem } => {
            assert_eq!(role, Role::Developer);
            assert!(problem.contains("may not send work"), "{problem}");
        }
        other => panic!("expected RoleFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn usage_limit_pauses_without_changing_anything() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    let before = state.clone();
    let agent = MockAgent::new().then(Role::Architect, MockStep::UsageLimit);

    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(stop, StopReason::UsageLimitReached(Role::Architect));
    assert_eq!(state, before);
}

#[tokio::test]
async fn continues_after_a_restart() {
    let (_dir, repo) = new_project();
    {
        let (store, mut state) = new_task(&repo);
        let agent = MockAgent::new().then(Role::Architect, approve(Role::Human));
        orchestrator::run(&repo, &store, &mut state, &agent)
            .await
            .unwrap();
    } // program "stops" here

    let (store, mut state) = TaskStore::open(&repo.runs_dir(), "task-001").unwrap();
    record_human_decision(
        &repo,
        &store,
        &mut state,
        Verdict::Approved,
        NextStep::To(Role::Developer),
        "",
    )
    .unwrap();
    let agent = MockAgent::new().then(
        Role::Developer,
        MockStep::finish(Verdict::NeedsHuman, NextStep::To(Role::Human)),
    );
    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(
        stop,
        StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Developer))
    );
    assert_eq!(store.history().unwrap().len(), 3);
}

// ---------- git checks after every role ----------

#[tokio::test]
async fn the_work_of_each_role_is_committed_with_its_handoff() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    state.stage = Stage::Working(Role::Developer);
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
            MockStep::finish_writing(
                Verdict::NeedsHuman,
                NextStep::To(Role::Human),
                &[("tests/parser.rs", "#[test] fn t() {}")],
            ),
        );

    orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert!(repo.changed_files().unwrap().is_empty());
    let show = Command::new("git")
        .current_dir(repo.root())
        .args(["show", "--name-only", "--format=%s", "HEAD~1"])
        .output()
        .unwrap();
    let show = String::from_utf8(show.stdout).unwrap();
    // The developer's commit holds its code and its handoff, nothing else.
    assert!(
        show.starts_with("task-001 round 1: developer (approved)"),
        "{show}"
    );
    assert!(show.contains("src/parser.rs"), "{show}");
    assert!(show.contains(".harness/runs/task-001/round-01/01-developer/handoff.json"));
    assert!(!show.contains("tests/parser.rs"), "{show}");
    assert!(!show.contains("inbox"), "{show}");
}

#[tokio::test]
async fn an_architect_changing_code_is_stopped_and_the_change_is_kept() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    let before = state.clone();
    let agent = MockAgent::new().then(
        Role::Architect,
        MockStep::finish_writing(
            Verdict::Approved,
            NextStep::To(Role::Human),
            &[("docs/design.md", "ok"), ("src/main.rs", "not yours")],
        ),
    );

    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(
        stop,
        StopReason::ForbiddenChanges {
            role: Role::Architect,
            files: vec!["src/main.rs".to_string()],
        }
    );
    assert_eq!(state, before);
    assert!(store.history().unwrap().is_empty());
    // Left for Lisa to look at.
    assert!(repo.root().join("src/main.rs").exists());

    // Until she cleans up, the harness refuses to start the next role.
    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();
    assert_eq!(
        stop,
        StopReason::DirtyWorkingTree(vec![
            "docs/design.md".to_string(),
            "src/main.rs".to_string()
        ])
    );
}

#[tokio::test]
async fn security_may_not_change_anything() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    state.stage = Stage::Working(Role::Security);
    let agent = MockAgent::new().then(
        Role::Security,
        MockStep::finish_writing(
            Verdict::Approved,
            NextStep::Done,
            &[("docs/security.md", "all fine")],
        ),
    );

    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert!(matches!(
        stop,
        StopReason::ForbiddenChanges {
            role: Role::Security,
            ..
        }
    ));
}

#[tokio::test]
async fn no_role_may_touch_the_harness_state() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    state.stage = Stage::Working(Role::Developer);
    let agent = MockAgent::new().then(
        Role::Developer,
        MockStep::finish_writing(
            Verdict::Approved,
            NextStep::To(Role::Tester),
            &[(".harness/runs/task-001/state.json", "{}")],
        ),
    );

    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(
        stop,
        StopReason::ForbiddenChanges {
            role: Role::Developer,
            files: vec![".harness/runs/task-001/state.json".to_string()],
        }
    );
}

#[tokio::test]
async fn a_failed_attempt_is_rolled_back_before_the_retry() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    state.stage = Stage::Working(Role::Developer);
    let agent = MockAgent::new()
        .then(
            Role::Developer,
            MockStep::WriteFilesOnly(vec![
                ("src/half_done.rs".to_string(), "broken".to_string()),
                ("README.md".to_string(), "broken".to_string()),
            ]),
        )
        .then(
            Role::Developer,
            MockStep::finish_writing(
                Verdict::NeedsHuman,
                NextStep::To(Role::Human),
                &[("src/parser.rs", "fn parse() {}")],
            ),
        );

    orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(agent.calls(), [Role::Developer, Role::Developer]);
    assert!(!repo.root().join("src/half_done.rs").exists());
    let readme = fs::read_to_string(repo.root().join("README.md")).unwrap();
    assert_eq!(readme, "A parser\n");
    assert!(repo.root().join("src/parser.rs").exists());
    assert!(repo.changed_files().unwrap().is_empty());
}

#[tokio::test]
async fn uncommitted_changes_stop_the_run_before_any_role() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    fs::write(repo.root().join("README.md"), "Lisa was editing").unwrap();
    let agent = MockAgent::new().then(Role::Architect, approve(Role::Human));

    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(
        stop,
        StopReason::DirtyWorkingTree(vec!["README.md".to_string()])
    );
    assert!(agent.calls().is_empty());
}

#[tokio::test]
async fn an_agent_that_commits_by_itself_is_stopped() {
    let (_dir, repo) = new_project();
    let (store, mut state) = new_task(&repo);
    let agent = MockAgent::new().then(Role::Architect, MockStep::GitCommit);

    let stop = orchestrator::run(&repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(stop, StopReason::AgentCommitted(Role::Architect));
}
