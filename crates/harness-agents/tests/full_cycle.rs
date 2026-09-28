//! Runs the whole loop of four roles with the mock agent.
//! Files in `tests/` are integration tests: they use the crates from outside,
//! exactly like the CLI will.

use harness_agents::{MockAgent, MockStep};
use harness_core::handoff::{NextStep, Role, Verdict};
use harness_core::orchestrator::{self, record_human_decision, StopReason, ATTEMPTS_PER_ROLE};
use harness_core::store::TaskStore;
use harness_core::task::{Stage, TaskState, WaitReason, DEFAULT_MAX_ROUNDS};

fn approve(next: Role) -> MockStep {
    MockStep::finish(Verdict::Approved, NextStep::To(next))
}

fn new_task(runs: &std::path::Path) -> (TaskStore, TaskState) {
    TaskStore::create(runs, "task-001", "Build a parser", DEFAULT_MAX_ROUNDS).unwrap()
}

#[tokio::test]
async fn happy_path_with_lisa_approving_the_design() {
    let runs = tempfile::tempdir().unwrap();
    let (store, mut state) = new_task(runs.path());
    let agent = MockAgent::new()
        .then(Role::Architect, approve(Role::Human))
        .then(Role::Developer, approve(Role::Tester))
        .then(Role::Tester, approve(Role::Security))
        .then(
            Role::Security,
            MockStep::finish(Verdict::Approved, NextStep::Done),
        );

    let stop = orchestrator::run(&store, &mut state, &agent).await.unwrap();
    assert_eq!(stop, StopReason::WaitingForHuman(WaitReason::ApproveDesign));

    record_human_decision(
        &store,
        &mut state,
        Verdict::Approved,
        NextStep::To(Role::Developer),
        "Looks good.",
    )
    .unwrap();

    let stop = orchestrator::run(&store, &mut state, &agent).await.unwrap();
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
}

#[tokio::test]
async fn tester_sends_a_bug_back_and_the_developer_fixes_it() {
    let runs = tempfile::tempdir().unwrap();
    let (store, mut state) = new_task(runs.path());
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

    let stop = orchestrator::run(&store, &mut state, &agent).await.unwrap();

    assert_eq!(stop, StopReason::Done);
    assert_eq!(state.round, 2);
    assert!(store.dir().join("round-01/02-tester").exists());
    assert!(store.dir().join("round-02/01-developer").exists());
}

#[tokio::test]
async fn a_bad_first_attempt_is_retried() {
    let runs = tempfile::tempdir().unwrap();
    let (store, mut state) = new_task(runs.path());
    let agent = MockAgent::new()
        .then(Role::Architect, MockStep::WriteGarbage)
        .then(Role::Architect, approve(Role::Human));

    let stop = orchestrator::run(&store, &mut state, &agent).await.unwrap();

    assert_eq!(stop, StopReason::WaitingForHuman(WaitReason::ApproveDesign));
    assert_eq!(agent.calls(), [Role::Architect, Role::Architect]);
}

#[tokio::test]
async fn a_role_that_keeps_failing_stops_the_task() {
    let runs = tempfile::tempdir().unwrap();
    let (store, mut state) = new_task(runs.path());
    let before = state.clone();
    let agent = MockAgent::new(); // no script: every run writes nothing

    let stop = orchestrator::run(&store, &mut state, &agent).await.unwrap();

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
    let runs = tempfile::tempdir().unwrap();
    let (store, mut state) = new_task(runs.path());
    state.stage = Stage::Working(Role::Developer);
    // The developer tries to skip the tester, twice.
    let agent = MockAgent::new()
        .then(Role::Developer, approve(Role::Security))
        .then(Role::Developer, approve(Role::Security));

    let stop = orchestrator::run(&store, &mut state, &agent).await.unwrap();

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
    let runs = tempfile::tempdir().unwrap();
    let (store, mut state) = new_task(runs.path());
    let before = state.clone();
    let agent = MockAgent::new().then(Role::Architect, MockStep::UsageLimit);

    let stop = orchestrator::run(&store, &mut state, &agent).await.unwrap();

    assert_eq!(stop, StopReason::UsageLimitReached(Role::Architect));
    assert_eq!(state, before);
}

#[tokio::test]
async fn continues_after_a_restart() {
    let runs = tempfile::tempdir().unwrap();
    {
        let (store, mut state) = new_task(runs.path());
        let agent = MockAgent::new().then(Role::Architect, approve(Role::Human));
        orchestrator::run(&store, &mut state, &agent).await.unwrap();
    } // program "stops" here

    let (store, mut state) = TaskStore::open(runs.path(), "task-001").unwrap();
    record_human_decision(
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
    let stop = orchestrator::run(&store, &mut state, &agent).await.unwrap();

    assert_eq!(
        stop,
        StopReason::WaitingForHuman(WaitReason::RoleAskedForHelp(Role::Developer))
    );
    assert_eq!(store.history().unwrap().len(), 3);
}
