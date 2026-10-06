//! Runs the Codex adapter against a fake `codex`: a small shell script.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use harness_agents::role_settings::RoleSettings;
use harness_agents::Codex;
use harness_core::git::Repo;
use harness_core::task::handoff::Role;
use harness_core::task::orchestrator::{self, create_task, StopReason};
use harness_core::task::store::TaskStore;
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
    /// Plays `~/.harness/credentials/codex`.
    auth_dir: PathBuf,
}

fn setup() -> Setup {
    let project = tempfile::tempdir().unwrap();
    let repo = Repo::init(project.path()).unwrap();
    fs::write(project.path().join("README.md"), "app\n").unwrap();
    repo.commit_all("first").unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let auth_dir = scratch.path().join("creds/codex");
    fs::create_dir_all(&auth_dir).unwrap();
    fs::write(auth_dir.join("auth.json"), r#"{"tokens":"old"}"#).unwrap();
    Setup {
        _project: project,
        scratch,
        repo,
        auth_dir,
    }
}

fn fake_codex(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("codex");
    fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn new_task(repo: &Repo) -> (TaskStore, TaskState) {
    create_task(repo, "task-001", "Design a parser", DEFAULT_MAX_ROUNDS).unwrap()
}

#[tokio::test]
async fn a_well_behaved_codex_finishes_the_role_and_keeps_a_refreshed_login() {
    let s = setup();
    let seen = s.scratch.path().join("seen");
    // The fake records its environment and whether the login was there, then
    // "refreshes" the login like Codex does, and does the architect's job.
    let script = fake_codex(
        s.scratch.path(),
        &format!(
            "env > {seen}.env\n\
             cat > {seen}.prompt\n\
             cp \"$CODEX_HOME/auth.json\" {seen}.auth\n\
             echo '{{\"tokens\":\"new\"}}' > \"$CODEX_HOME/auth.json\"\n\
             cat > .harness/runs/task-001/inbox/handoff.json <<'JSON'\n{HANDOFF}\nJSON\n\
             echo '{{\"type\":\"thread.started\"}}'\n\
             echo '{{\"type\":\"turn.completed\",\"usage\":{{}}}}'\n",
            seen = seen.display()
        ),
    );
    std::env::set_var("OPENAI_API_KEY", "must-not-leak");
    let agent = Codex::new(&s.auth_dir, RoleSettings::default()).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(stop, StopReason::WaitingForHuman(WaitReason::ApproveDesign));
    let env = fs::read_to_string(seen.with_extension("env")).unwrap();
    assert!(env.contains("CODEX_HOME="), "{env}");
    assert!(!env.contains("must-not-leak"), "{env}");
    assert!(fs::read_to_string(seen.with_extension("prompt"))
        .unwrap()
        .contains("Design a parser"));
    // The login was there during the run...
    assert_eq!(
        fs::read_to_string(seen.with_extension("auth")).unwrap(),
        r#"{"tokens":"old"}"#
    );
    // ...is gone from the project afterwards, and the refreshed one was kept.
    assert!(!s
        .repo
        .root()
        .join(".harness/agents/codex/auth.json")
        .exists());
    let saved = fs::read_to_string(s.auth_dir.join("auth.json")).unwrap();
    assert!(saved.contains("new"), "{saved}");

    let log = fs::read_to_string(store.dir().join("round-01/01-architect/agent.log")).unwrap();
    assert!(log.starts_with("agent: codex, model: default"), "{log}");
    assert!(s.repo.changed_files().unwrap().is_empty());
}

#[tokio::test]
async fn a_used_up_subscription_pauses_the_task() {
    let s = setup();
    let script = fake_codex(
        s.scratch.path(),
        "cat > /dev/null\n\
         echo '{\"type\":\"turn.failed\",\"error\":{\"message\":\"You have hit your usage limit.\"}}'\n\
         exit 1\n",
    );
    let agent = Codex::new(&s.auth_dir, RoleSettings::default()).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(stop, StopReason::UsageLimitReached(Role::Architect));
}

#[tokio::test]
async fn without_a_saved_login_the_role_fails_with_a_hint() {
    let s = setup();
    fs::remove_file(s.auth_dir.join("auth.json")).unwrap();
    let script = fake_codex(s.scratch.path(), "exit 0\n");
    let agent = Codex::new(&s.auth_dir, RoleSettings::default()).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    match stop {
        StopReason::RoleFailed { problem, .. } => {
            assert!(problem.contains("sign in on the Agents tab"), "{problem}");
        }
        other => panic!("expected RoleFailed, got {other:?}"),
    }
}
