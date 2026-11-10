//! A whole ACP conversation with a fake agent that answers like Gemini CLI
//! does: the fake prints the protocol's lines and saves what the harness sent.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use harness_core::config::AcpConfig;
use harness_core::mcp::McpServer;
use harness_core::secret::Secret;
use harness_core::task::agent::{AgentRunner, RoleJob, RunEnd};
use harness_core::task::handoff::Role;
use harness_core::task::stop_request::StopRequest;
use serde_json::Value;

use super::Acp;
use crate::adapters::gemini::{Gemini, LOGIN_FILE, SETTINGS_DIR};
use crate::role_settings::RoleSettings;

const HELLO: &str = r#"{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":1,"authMethods":[]}}"#;
const SESSION: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s1"}}"#;
const UPDATE: &str = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Working on it"}}}}"#;
const ASK_PUSH: &str = r#"{"jsonrpc":"2.0","id":"p1","method":"session/request_permission","params":{"sessionId":"s1","toolCall":{"toolCallId":"t1","title":"git push https://key-12345678@host","kind":"execute","rawInput":{"command":"git push https://key-12345678@host"}},"options":[{"optionId":"yes","name":"Allow","kind":"allow_once"},{"optionId":"no","name":"Reject","kind":"reject_once"}]}}"#;
const ASK_FILE: &str = r#"{"jsonrpc":"2.0","id":9,"method":"fs/read_text_file","params":{"sessionId":"s1","path":"/etc/passwd"}}"#;

/// The folder of one test: the project, the saved lines and the fake agent.
struct Setup {
    dir: tempfile::TempDir,
    project: PathBuf,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir_all(project.join("out")).unwrap();
        Self { dir, project }
    }

    /// A file next to the project where the fake saves what it got.
    fn saved(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn read_json(&self, name: &str) -> Value {
        let text = fs::read_to_string(self.saved(name)).unwrap();
        serde_json::from_str(text.trim()).unwrap()
    }

    /// The fake agent: `script`, with `{saved}` standing for the saved-files folder.
    fn agent(&self, script: &str) -> PathBuf {
        let saved = self.dir.path().to_string_lossy().replace('\\', "/");
        let script = script.replace("{saved}", &saved);
        harness_fake::install(&self.dir.path().join("bin"), "agent", &script)
    }

    fn job(&self, role: Role) -> RoleJob {
        RoleJob {
            task_id: "task-001".into(),
            round: 1,
            role,
            project_dir: self.project.clone(),
            prompt: "Design a parser".into(),
            output_dir: self.project.join("out"),
            stop: StopRequest::new(),
        }
    }
}

fn settings(role: Role) -> RoleSettings {
    let server = McpServer {
        name: "context7".into(),
        command: "npx".into(),
        args: vec!["ctx".into()],
        env: BTreeMap::from([("KEY".to_string(), Secret::new("key-12345678"))]),
    };
    RoleSettings::default()
        .with_timeout(Duration::from_secs(20))
        .with_mcp_servers(role, vec![server])
}

fn acp(program: &Path, role: Role) -> Acp {
    let config = AcpConfig {
        command: "never-used".into(),
        args: vec!["acp".into()],
    };
    Acp::new(&config, settings(role)).with_program(program)
}

#[tokio::test]
async fn a_whole_turn_opens_a_session_sends_the_prompt_and_refuses_what_the_role_may_not() {
    let s = Setup::new();
    let script = format!(
        "save-args {{saved}}/args.txt\nread-line\nprint {HELLO}\n\
         save-line {{saved}}/new.json\nprint {SESSION}\n\
         save-line {{saved}}/prompt.json\nprint {UPDATE}\nprint {ASK_PUSH}\n\
         save-line {{saved}}/permission.json\nprint {ASK_FILE}\n\
         save-line {{saved}}/file.json\nprint MCP tool echoed key-12345678\n\
         print {{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"stopReason\":\"end_turn\"}}}}\n"
    );
    let program = s.agent(&script);

    let outcome = acp(&program, Role::Developer)
        .run(&s.job(Role::Developer))
        .await;

    assert_eq!(outcome.end, RunEnd::Succeeded, "{}", outcome.log);
    assert_eq!(fs::read_to_string(s.saved("args.txt")).unwrap(), "acp\n");
    let new = s.read_json("new.json");
    assert_eq!(new["method"], "session/new");
    assert_eq!(
        Path::new(new["params"]["cwd"].as_str().unwrap()),
        s.project.as_path()
    );
    assert_eq!(new["params"]["mcpServers"][0]["name"], "context7");
    let prompt = s.read_json("prompt.json");
    assert_eq!(prompt["params"]["prompt"][0]["text"], "Design a parser");
    assert_eq!(prompt["params"]["sessionId"], "s1");
    // `git push` is for the harness alone.
    let permission = s.read_json("permission.json");
    assert_eq!(permission["id"], "p1");
    assert_eq!(permission["result"]["outcome"]["optionId"], "no");
    // The harness reads no files for the agent.
    assert_eq!(s.read_json("file.json")["error"]["code"], -32601);
    assert!(
        outcome.log.contains("harness refused git push"),
        "{}",
        outcome.log
    );
    assert!(outcome.log.contains("Working on it"), "{}", outcome.log);
    assert!(!outcome.log.contains("key-12345678"), "{}", outcome.log);
}

#[tokio::test]
async fn an_agent_that_is_not_signed_in_says_how_to_sign_in() {
    let s = Setup::new();
    let script = format!(
        "read-line\nprint {HELLO}\nread-line\n\
         print {{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{{\"code\":-32000,\"message\":\"Gemini API key is missing\"}}}}\n"
    );
    let program = s.agent(&script);

    let outcome = acp(&program, Role::Architect)
        .run(&s.job(Role::Architect))
        .await;

    let RunEnd::Failed(message) = &outcome.end else {
        panic!("{:?}", outcome.end);
    };
    assert!(message.contains("Gemini API key is missing"), "{message}");
    assert!(message.contains("sign in"), "{message}");
}

#[tokio::test]
async fn a_turn_that_ends_early_or_an_agent_that_quits_fails_the_role() {
    let s = Setup::new();
    let refused = s.agent(&format!(
        "read-line\nprint {HELLO}\nread-line\nprint {SESSION}\nread-line\n\
         print {{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"stopReason\":\"refusal\"}}}}\n"
    ));
    let outcome = acp(&refused, Role::Tester).run(&s.job(Role::Tester)).await;
    assert!(
        matches!(&outcome.end, RunEnd::Failed(m) if m.contains("refusal")),
        "{:?}",
        outcome.end
    );

    let quits = s.agent(&format!("read-line\nprint {HELLO}\nread-line\nexit 3\n"));
    let outcome = acp(&quits, Role::Tester).run(&s.job(Role::Tester)).await;
    assert!(
        matches!(&outcome.end, RunEnd::Failed(m) if m.contains("ended before it answered")),
        "{:?}",
        outcome.end
    );

    let other_version = s.agent(
        "read-line\nprint {\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"protocolVersion\":2}}\n",
    );
    let outcome = acp(&other_version, Role::Tester)
        .run(&s.job(Role::Tester))
        .await;
    assert!(
        matches!(&outcome.end, RunEnd::Failed(m) if m.contains("version 2")),
        "{:?}",
        outcome.end
    );
}

#[tokio::test]
async fn a_used_up_subscription_pauses_the_task() {
    let s = Setup::new();
    let program = s.agent(&format!(
        "read-line\nprint {HELLO}\nread-line\nprint {SESSION}\nread-line\n\
         print {{\"jsonrpc\":\"2.0\",\"id\":2,\"error\":{{\"code\":429,\"message\":\"You have exhausted your daily quota\"}}}}\n"
    ));

    let outcome = acp(&program, Role::Developer)
        .run(&s.job(Role::Developer))
        .await;

    assert_eq!(outcome.end, RunEnd::UsageLimit, "{}", outcome.log);
}

#[tokio::test]
async fn gemini_runs_in_a_home_of_its_own_with_the_saved_login() {
    let s = Setup::new();
    let auth = s.dir.path().join("auth");
    fs::create_dir_all(auth.join(SETTINGS_DIR)).unwrap();
    fs::write(auth.join(SETTINGS_DIR).join(LOGIN_FILE), "{}").unwrap();
    let program = s.agent(&format!(
        "save-env {{saved}}/env.txt\nsave-args {{saved}}/args.txt\n\
         need-file ${{HOME}}/.gemini/{LOGIN_FILE} else 7\n\
         read-line\nprint {HELLO}\nread-line\nprint {SESSION}\nread-line\n\
         print {{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"stopReason\":\"end_turn\"}}}}\n"
    ));
    let gemini = Gemini::new(&auth, settings(Role::Developer)).with_program(&program);

    let outcome = gemini.run(&s.job(Role::Developer)).await;

    assert_eq!(outcome.end, RunEnd::Succeeded, "{}", outcome.log);
    let args = fs::read_to_string(s.saved("args.txt")).unwrap();
    assert!(args.starts_with("--acp\n"), "{args}");
    let env = fs::read_to_string(s.saved("env.txt")).unwrap();
    let home = env
        .lines()
        .find_map(|line| line.strip_prefix("HOME="))
        .unwrap_or_default();
    assert!(home.contains("harness-gemini-"), "{env}");
    // The temporary home is gone after the role.
    assert!(!Path::new(home).exists());
    assert!(!env.contains("key-12345678"), "{env}");
}
