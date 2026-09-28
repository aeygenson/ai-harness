//! Runs the Gemini adapter against a fake `gemini`: a small shell script.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use harness_agents::Gemini;
use harness_core::git::Repo;
use harness_core::handoff::Role;
use harness_core::orchestrator::{self, create_task, StopReason};
use harness_core::store::TaskStore;
use harness_core::task::{TaskState, WaitReason, DEFAULT_MAX_ROUNDS};
use tempfile::TempDir;

const HANDOFF: &str = r#"{
  "schema_version": 1, "task_id": "task-001", "round": 1, "role": "architect",
  "verdict": "approved", "next_role": "human", "summary": "Design written",
  "skills_used": [], "files": [], "issues": []
}"#;

struct Setup {
    _project: TempDir,
    scratch: TempDir,
    repo: Repo,
    /// Plays `~/.harness/credentials/gemini`.
    auth_dir: PathBuf,
}

fn setup() -> Setup {
    let project = tempfile::tempdir().unwrap();
    let repo = Repo::init(project.path()).unwrap();
    fs::write(project.path().join("README.md"), "app\n").unwrap();
    repo.commit_all("first").unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let auth_dir = scratch.path().join("creds/gemini");
    fs::create_dir_all(auth_dir.join(".gemini")).unwrap();
    fs::write(
        auth_dir.join(".gemini/oauth_creds.json"),
        r#"{"access_token":"old"}"#,
    )
    .unwrap();
    Setup {
        _project: project,
        scratch,
        repo,
        auth_dir,
    }
}

fn fake_gemini(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("gemini");
    fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn new_task(repo: &Repo) -> (TaskStore, TaskState) {
    create_task(repo, "task-001", "Design a parser", DEFAULT_MAX_ROUNDS).unwrap()
}

#[tokio::test]
async fn a_well_behaved_gemini_finishes_the_role_and_keeps_a_refreshed_login() {
    let s = setup();
    let seen = s.scratch.path().join("seen");
    // The fake records its environment, the login and the policy it got, then
    // "refreshes" the login like Gemini does, and does the architect's job.
    let script = fake_gemini(
        s.scratch.path(),
        &format!(
            "env > {seen}.env\n\
             cat > {seen}.prompt\n\
             cp \"$GEMINI_CLI_HOME/.gemini/oauth_creds.json\" {seen}.auth\n\
             cp \"$GEMINI_CLI_HOME/policy.toml\" {seen}.policy\n\
             echo '{{\"access_token\":\"new\"}}' > \"$GEMINI_CLI_HOME/.gemini/oauth_creds.json\"\n\
             cat > .harness/runs/task-001/inbox/handoff.json <<'JSON'\n{HANDOFF}\nJSON\n\
             echo '{{\"type\":\"init\"}}'\n\
             echo '{{\"type\":\"result\",\"status\":\"success\",\"stats\":{{}}}}'\n",
            seen = seen.display()
        ),
    );
    std::env::set_var("GEMINI_API_KEY", "must-not-leak");
    let agent = Gemini::new(&s.auth_dir).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(stop, StopReason::WaitingForHuman(WaitReason::ApproveDesign));
    let env = fs::read_to_string(seen.with_extension("env")).unwrap();
    assert!(env.contains("GEMINI_CLI_HOME="), "{env}");
    assert!(!env.contains("must-not-leak"), "{env}");
    assert!(fs::read_to_string(seen.with_extension("prompt"))
        .unwrap()
        .contains("Design a parser"));
    let policy = fs::read_to_string(seen.with_extension("policy")).unwrap();
    assert!(policy.contains("The Architect only reads"), "{policy}");
    // The login was there during the run...
    assert_eq!(
        fs::read_to_string(seen.with_extension("auth")).unwrap(),
        r#"{"access_token":"old"}"#
    );
    // ...is gone from the project afterwards, and the refreshed one was kept.
    assert!(!s
        .repo
        .root()
        .join(".harness/agents/gemini/.gemini/oauth_creds.json")
        .exists());
    let saved = fs::read_to_string(s.auth_dir.join(".gemini/oauth_creds.json")).unwrap();
    assert!(saved.contains("new"), "{saved}");

    let log = fs::read_to_string(store.dir().join("round-01/01-architect/agent.log")).unwrap();
    assert!(log.starts_with("agent: gemini, model: default"), "{log}");
    assert!(s.repo.changed_files().unwrap().is_empty());
}

#[tokio::test]
async fn a_used_up_quota_pauses_the_task() {
    let s = setup();
    let script = fake_gemini(
        s.scratch.path(),
        "cat > /dev/null\n\
         echo '{\"type\":\"result\",\"status\":\"error\",\"error\":{\"type\":\"TerminalQuotaError\",\
         \"message\":\"You have exhausted your daily quota on this model.\"}}'\n\
         exit 1\n",
    );
    let agent = Gemini::new(&s.auth_dir).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(stop, StopReason::UsageLimitReached(Role::Architect));
}

#[tokio::test]
async fn a_failed_login_is_shown_from_stderr() {
    let s = setup();
    // What the real Gemini CLI does when its login cannot be refreshed.
    let script = fake_gemini(
        s.scratch.path(),
        "cat > /dev/null\n\
         echo 'Manual authorization is required but the current session is non-interactive.' >&2\n\
         exit 41\n",
    );
    let agent = Gemini::new(&s.auth_dir).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    match stop {
        StopReason::RoleFailed { problem, .. } => {
            assert!(problem.contains("Manual authorization"), "{problem}")
        }
        other => panic!("expected RoleFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn without_a_saved_login_the_role_fails_with_a_hint() {
    let s = setup();
    fs::remove_file(s.auth_dir.join(".gemini/oauth_creds.json")).unwrap();
    let script = fake_gemini(s.scratch.path(), "exit 0\n");
    let agent = Gemini::new(&s.auth_dir).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    match stop {
        StopReason::RoleFailed { problem, .. } => {
            assert!(problem.contains("harness login gemini"), "{problem}")
        }
        other => panic!("expected RoleFailed, got {other:?}"),
    }
}
