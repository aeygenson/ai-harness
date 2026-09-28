//! Runs the Claude Code adapter against a fake `claude`: a small shell script.
//! It checks the real process handling (environment, standard input, time-out,
//! reading the output) without a subscription or internet.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use harness_agents::credentials::Secret;
use harness_agents::ClaudeCode;
use harness_core::git::Repo;
use harness_core::handoff::Role;
use harness_core::orchestrator::{self, create_task, StopReason};
use harness_core::task::{TaskState, WaitReason, DEFAULT_MAX_ROUNDS};
use tempfile::TempDir;

const HANDOFF: &str = r#"{
  "schema_version": 1, "task_id": "task-001", "round": 1, "role": "architect",
  "verdict": "approved", "next_role": "human", "summary": "Design written",
  "skills_used": [], "files": [{"path": "docs/design.md", "action": "created"}], "issues": []
}"#;

struct Setup {
    _project: TempDir,
    scratch: TempDir,
    repo: Repo,
}

fn setup() -> Setup {
    let project = tempfile::tempdir().unwrap();
    let repo = Repo::init(project.path()).unwrap();
    fs::write(project.path().join("README.md"), "app\n").unwrap();
    repo.commit_all("first").unwrap();
    Setup {
        _project: project,
        scratch: tempfile::tempdir().unwrap(),
        repo,
    }
}

/// Writes an executable script and returns its path.
fn fake_claude(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("claude");
    fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn new_task(repo: &Repo) -> (harness_core::store::TaskStore, TaskState) {
    create_task(repo, "task-001", "Design a parser", DEFAULT_MAX_ROUNDS).unwrap()
}

#[tokio::test]
async fn a_well_behaved_agent_finishes_the_role() {
    let s = setup();
    let seen = s.scratch.path().join("seen");
    // The fake saves what it received, then does the architect's job.
    let script = fake_claude(
        s.scratch.path(),
        &format!(
            "env > {seen}.env\n\
             cat > {seen}.prompt\n\
             mkdir -p docs && echo '# Design' > docs/design.md\n\
             cat > .harness/runs/task-001/inbox/handoff.json <<'JSON'\n{HANDOFF}\nJSON\n\
             echo 'Design notes' > .harness/runs/task-001/inbox/notes.md\n\
             echo '{{\"type\":\"result\",\"is_error\":false,\"result\":\"Done.\"}}'\n",
            seen = seen.display()
        ),
    );
    std::env::set_var("SECRET_TEST_API_KEY", "must-not-leak");
    let agent = ClaudeCode::new(Secret::new("tok-123")).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(stop, StopReason::WaitingForHuman(WaitReason::ApproveDesign));
    let env = fs::read_to_string(seen.with_extension("env")).unwrap();
    assert!(env.contains("CLAUDE_CODE_OAUTH_TOKEN=tok-123"));
    assert!(env.contains("CLAUDE_CONFIG_DIR="));
    assert!(!env.contains("must-not-leak"), "{env}");
    let prompt = fs::read_to_string(seen.with_extension("prompt")).unwrap();
    assert!(prompt.contains("Design a parser"));

    let step = store.dir().join("round-01/01-architect");
    let log = fs::read_to_string(step.join("agent.log")).unwrap();
    assert!(log.starts_with("agent: claude, model: default"), "{log}");
    assert!(log.contains("\"result\":\"Done.\""));
    assert!(!log.contains("tok-123"));
    // Committed, and the agent's own settings folder stays out of git.
    assert!(s.repo.changed_files().unwrap().is_empty());
    assert!(s
        .repo
        .root()
        .join(".harness/agents/claude/settings.json")
        .exists());
}

#[tokio::test]
async fn a_used_up_subscription_pauses_the_task() {
    let s = setup();
    let script = fake_claude(
        s.scratch.path(),
        "cat > /dev/null\n\
         echo '{\"type\":\"result\",\"is_error\":true,\"result\":\"Claude AI usage limit reached\"}'\n\
         exit 1\n",
    );
    let agent = ClaudeCode::new(Secret::new("t")).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(stop, StopReason::UsageLimitReached(Role::Architect));
}

#[tokio::test]
async fn a_hanging_agent_is_stopped_by_the_time_out() {
    let s = setup();
    let script = fake_claude(s.scratch.path(), "sleep 10\n");
    let agent = ClaudeCode::new(Secret::new("t"))
        .with_program(script)
        .with_timeout(Duration::from_millis(300));
    let (store, mut state) = new_task(&s.repo);

    let started = std::time::Instant::now();
    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    match stop {
        StopReason::RoleFailed { role, problem } => {
            assert_eq!(role, Role::Architect);
            assert!(problem.contains("timed out"), "{problem}");
        }
        other => panic!("expected RoleFailed, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn a_missing_claude_program_is_a_role_failure() {
    let s = setup();
    let agent = ClaudeCode::new(Secret::new("t")).with_program("/no/such/claude");
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    match stop {
        StopReason::RoleFailed { problem, .. } => {
            assert!(problem.contains("cannot start"), "{problem}")
        }
        other => panic!("expected RoleFailed, got {other:?}"),
    }
}
