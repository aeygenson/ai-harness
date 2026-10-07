//! Runs the Antigravity adapter against a fake `agy` (`harness-fake`).

use std::fs;
use std::path::{Path, PathBuf};

use harness_agents::role_settings::RoleSettings;
use harness_agents::Antigravity;
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
    /// Plays `~/.harness/credentials/antigravity`.
    auth_dir: PathBuf,
}

fn setup() -> Setup {
    let project = tempfile::tempdir().unwrap();
    let repo = Repo::init(project.path()).unwrap();
    fs::write(project.path().join("README.md"), "app\n").unwrap();
    repo.commit_all("first").unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let auth_dir = scratch.path().join("creds/antigravity");
    let settings = auth_dir.join(".gemini/antigravity-cli");
    fs::create_dir_all(settings.join("implicit")).unwrap();
    fs::create_dir_all(settings.join("conversations")).unwrap();
    fs::write(settings.join("implicit/login.pb"), "login").unwrap();
    fs::write(settings.join("conversations/old.db"), "history").unwrap();
    fs::write(settings.join("settings.json"), "{}").unwrap();
    Setup {
        _project: project,
        scratch,
        repo,
        auth_dir,
    }
}

/// Puts a fake `agy` doing what `script` says into `dir`.
fn fake_agy(dir: &Path, script: &str) -> PathBuf {
    harness_fake::install(dir, "agy", script)
}

fn new_task(repo: &Repo) -> (TaskStore, TaskState) {
    create_task(repo, "task-001", "Design a parser", DEFAULT_MAX_ROUNDS).unwrap()
}

#[tokio::test]
async fn a_well_behaved_agy_finishes_the_role_in_a_throwaway_home() {
    let s = setup();
    let seen = s.scratch.path().join("seen");
    // The fake records what it got, then does the architect's job.
    let script = fake_agy(
        s.scratch.path(),
        &format!(
            "save-env {seen}.env\n\
             save-text {seen}.prompt $2\n\
             save-text {seen}.home ${{HOME}}\n\
             list ${{HOME}}/.gemini/antigravity-cli {seen}.files\n\
             copy ${{HOME}}/.gemini/antigravity-cli/settings.json {seen}.settings\n\
             write .harness/runs/task-001/inbox/handoff.json\n{HANDOFF}\nend\n\
             print {{\"event\":\"init\"}}\n\
             print {{\"event\":\"result\",\"result\":{{\"status\":\"SUCCESS\",\"response\":\"done\"}}}}\n",
            seen = seen.display()
        ),
    );
    std::env::set_var("GEMINI_API_KEY", "must-not-leak");
    let agent = Antigravity::new(&s.auth_dir, RoleSettings::default()).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(stop, StopReason::WaitingForHuman(WaitReason::ApproveDesign));
    let env = fs::read_to_string(seen.with_extension("env")).unwrap();
    assert!(!env.contains("must-not-leak"), "{env}");
    assert!(fs::read_to_string(seen.with_extension("prompt"))
        .unwrap()
        .contains("Design a parser"));
    // The login was copied, the old conversations were not.
    let files = fs::read_to_string(seen.with_extension("files")).unwrap();
    assert!(files.contains("implicit"), "{files}");
    assert!(!files.contains("conversations"), "{files}");
    let settings = fs::read_to_string(seen.with_extension("settings")).unwrap();
    assert!(settings.contains("command(git commit)"), "{settings}");
    // The throwaway HOME is gone, and it was never inside the project.
    let home = PathBuf::from(
        fs::read_to_string(seen.with_extension("home"))
            .unwrap()
            .trim(),
    );
    assert!(!home.exists(), "{}", home.display());
    assert!(!home.starts_with(s.repo.root()));

    let log = fs::read_to_string(store.dir().join("round-01/01-architect/agent.log")).unwrap();
    assert!(
        log.starts_with("agent: antigravity, model: default"),
        "{log}"
    );
    assert_eq!(s.repo.changed_files().unwrap(), Vec::<String>::new());
}

#[tokio::test]
async fn a_used_up_quota_pauses_the_task() {
    let s = setup();
    let script = fake_agy(
        s.scratch.path(),
        "eprint AGY_ERROR: {\"status\":\"RESOURCE_EXHAUSTED\",\"retryable\":false}\n\
         exit 3\n",
    );
    let agent = Antigravity::new(&s.auth_dir, RoleSettings::default()).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    assert_eq!(stop, StopReason::UsageLimitReached(Role::Architect));
}

#[tokio::test]
async fn a_run_cut_short_by_a_permission_says_so() {
    let s = setup();
    // What agy 1.2.12 printed when a command needed a permission it could not ask for.
    let script = fake_agy(
        s.scratch.path(),
        "print {\"event\":\"result\",\"result\":{\"status\":\"SUCCESS\",\"response\":\"\",\
         \"denied_actions\":[{\"action\":\"command\",\"display_name\":\"RunCommand\"}]}}\n",
    );
    let agent = Antigravity::new(&s.auth_dir, RoleSettings::default()).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    match stop {
        StopReason::RoleFailed { problem, .. } => {
            assert!(
                problem.contains("needed permission for RunCommand"),
                "{problem}"
            );
        }
        other => panic!("expected RoleFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_missing_login_is_shown_from_stderr() {
    let s = setup();
    let script = fake_agy(
        s.scratch.path(),
        "eprint error: authentication required\nexit 1\n",
    );
    let agent = Antigravity::new(&s.auth_dir, RoleSettings::default()).with_program(script);
    let (store, mut state) = new_task(&s.repo);

    let stop = orchestrator::run(&s.repo, &store, &mut state, &agent)
        .await
        .unwrap();

    match stop {
        StopReason::RoleFailed { problem, .. } => {
            assert!(problem.contains("authentication required"), "{problem}");
        }
        other => panic!("expected RoleFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn without_a_saved_login_the_role_fails_with_a_hint() {
    let s = setup();
    fs::remove_dir_all(&s.auth_dir).unwrap();
    let script = fake_agy(s.scratch.path(), "exit 0\n");
    let agent = Antigravity::new(&s.auth_dir, RoleSettings::default()).with_program(script);
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
