use super::*;
use harness_core::mcp::McpServer;
use harness_core::task::agent::RunEnd;

fn job(role: Role) -> RoleJob {
    RoleJob {
        task_id: "task-001".into(),
        round: 1,
        role,
        project_dir: PathBuf::from("/work/app"),
        prompt: "do it".into(),
        output_dir: PathBuf::from("/work/app/.harness/runs/task-001/inbox"),
        stop: harness_core::task::stop_request::StopRequest::new(),
    }
}

fn args(command: &Command) -> Vec<String> {
    command
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn env(command: &Command, name: &str) -> Option<String> {
    command
        .get_envs()
        .find(|(k, _)| *k == name)
        .and_then(|(_, v)| v)
        .map(|v| v.to_string_lossy().into_owned())
}

#[test]
fn cursor_runs_headless_forced_in_the_project_with_its_own_config() {
    let cursor = Cursor::new(RoleSettings::default().with_model(Role::Tester, "gpt-5"));
    let command = cursor.command(&job(Role::Tester), Path::new("/tmp/cfg"));
    let args = args(&command);
    assert_eq!(&args[..3], ["-p", "--output-format", "stream-json"]);
    for flag in ["--trust", "--force"] {
        assert!(args.contains(&flag.to_string()), "{args:?}");
    }
    let workspace = args.iter().position(|a| a == "--workspace").unwrap();
    assert_eq!(args[workspace + 1], "/work/app");
    let model = args.iter().position(|a| a == "--model").unwrap();
    assert_eq!(args[model + 1], "gpt-5");
    assert_eq!(args.last().map(String::as_str), Some("do it"));
    assert!(!args.contains(&"--approve-mcps".to_string()), "{args:?}");
    assert_eq!(
        env(&command, "CURSOR_CONFIG_DIR").as_deref(),
        Some("/tmp/cfg")
    );
    assert_eq!(env(&command, "NO_OPEN_BROWSER").as_deref(), Some("1"));
}

#[test]
fn mcp_servers_are_approved_only_when_the_role_has_some() {
    let server = McpServer {
        name: "docs".into(),
        command: "docs-server".into(),
        args: vec![],
        env: std::collections::BTreeMap::new(),
    };
    let settings = RoleSettings::default().with_mcp_servers(Role::Developer, vec![server]);
    let cursor = Cursor::new(settings);
    let with = args(&cursor.command(&job(Role::Developer), Path::new("/c")));
    let without = args(&cursor.command(&job(Role::Tester), Path::new("/c")));
    assert!(with.contains(&"--approve-mcps".to_string()), "{with:?}");
    assert!(
        !without.contains(&"--approve-mcps".to_string()),
        "{without:?}"
    );
}

#[test]
fn the_config_folder_holds_only_our_settings_and_is_deleted() {
    let cursor = Cursor::new(RoleSettings::default());
    let dir = cursor.prepare_config(Role::Security).unwrap();
    let path = dir.path().to_path_buf();
    let mut names: Vec<String> = fs::read_dir(&path)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["cli-config.json", "mcp.json"]);
    let config = fs::read_to_string(path.join("cli-config.json")).unwrap();
    assert!(config.contains("Shell(rm)"), "{config}");
    drop(dir);
    assert!(!path.exists());
}

#[tokio::test]
async fn a_run_succeeds_on_a_result_event_and_fails_with_cursors_message() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    // A fake Cursor: needs its config file and the prompt as the last argument.
    let program = harness_fake::install(
        &bin,
        "agent",
        "need-file ${CURSOR_CONFIG_DIR}/cli-config.json else 2\n\
         print {\"type\":\"system\",\"subtype\":\"init\"}\n\
         print {\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"done: ${8}\"}\n",
    );
    let mut job = job(Role::Tester);
    job.project_dir = dir.path().to_path_buf();
    let cursor = Cursor::new(RoleSettings::default()).with_program(&program);
    let outcome = cursor.run(&job).await;
    assert_eq!(outcome.end, RunEnd::Succeeded, "{}", outcome.log);
    assert!(outcome.log.contains("done: do it"), "{}", outcome.log);
    assert!(outcome.log.starts_with("agent: cursor"), "{}", outcome.log);

    harness_fake::install(&bin, "agent", "eprint Error: not logged in\nexit 1\n");
    let outcome = cursor.run(&job).await;
    let RunEnd::Failed(message) = outcome.end else {
        panic!("expected a failed run: {:?}", outcome.end);
    };
    assert!(message.contains("not logged in"), "{message}");

    harness_fake::install(
        &bin,
        "agent",
        "eprint Error: You've hit your usage limit\nexit 1\n",
    );
    assert_eq!(cursor.run(&job).await.end, RunEnd::UsageLimit);
}

#[test]
fn an_error_result_is_a_failure_with_its_text() {
    let result: Result<process::Finished, String> = Err("cannot start agent".into());
    let outcome = outcome(String::new(), result);
    assert_eq!(outcome.end, RunEnd::Failed("cannot start agent".into()));
}
