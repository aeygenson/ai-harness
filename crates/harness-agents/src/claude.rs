//! Runs a role with Claude Code (`claude -p`), isolated from Lisa's own setup.
//!
//! Isolation (docs/design.md, section 5.1):
//! - an empty environment plus a short whitelist, so API keys from the terminal
//!   never reach the agent;
//! - `CLAUDE_CONFIG_DIR` points to `<project>/.harness/agents/claude/`, so the
//!   agent does not see `~/.claude` (plugins, MCP servers, hooks, memory);
//! - login only through the token saved by `harness login claude`;
//! - the role's tools and file rules are given as flags, and anything not
//!   allowed is refused without asking (`--permission-mode dontAsk`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use harness_core::agent::{AgentOutcome, AgentRunner, RoleJob};
use harness_core::handoff::Role;
use tokio::io::AsyncWriteExt;

use crate::credentials::Secret;

/// Environment variables the agent may inherit from Lisa's terminal. Everything
/// else, including every `*_API_KEY`, is removed.
const INHERITED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "LC_ALL",
    "TERM",
    "TMPDIR",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
];

/// Where the agent's own settings live inside the project (ignored by git).
pub const CONFIG_DIR: &str = ".harness/agents/claude";

#[derive(Debug, Clone)]
pub struct ClaudeCode {
    program: PathBuf,
    token: Secret,
    models: HashMap<Role, String>,
    timeout: Duration,
}

impl ClaudeCode {
    pub fn new(token: Secret) -> Self {
        Self {
            program: PathBuf::from("claude"),
            token,
            models: HashMap::new(),
            timeout: Duration::from_secs(30 * 60),
        }
    }

    /// Which program to run instead of `claude` (tests use a fake script).
    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    pub fn with_model(mut self, role: Role, model: impl Into<String>) -> Self {
        self.models.insert(role, model.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Builds the full command for one job, without running it.
    /// A separate function, so tests can check every flag and variable.
    pub fn command(&self, job: &RoleJob) -> Command {
        let config_dir = job.project_dir.join(CONFIG_DIR);
        let rules = RoleRules::for_job(job);

        let mut command = Command::new(&self.program);
        command
            .current_dir(&job.project_dir)
            .env_clear()
            .envs(
                INHERITED_ENV
                    .iter()
                    .filter_map(|name| std::env::var_os(name).map(|value| (*name, value))),
            )
            .env("CLAUDE_CONFIG_DIR", &config_dir)
            .env("CLAUDE_CODE_OAUTH_TOKEN", self.token.expose())
            .env("DISABLE_AUTOUPDATER", "1")
            // The prompt comes on standard input.
            .arg("-p")
            .args(["--output-format", "stream-json", "--verbose"])
            .args(["--permission-mode", "dontAsk"])
            .args(["--tools", &rules.tools.join(",")])
            .args(["--allowedTools", &rules.allowed.join(",")])
            .args(["--disallowedTools", &rules.denied.join(",")])
            // No MCP servers at all, and settings only from our own config dir.
            .args([
                "--strict-mcp-config",
                "--mcp-config",
                r#"{"mcpServers":{}}"#,
            ])
            .args(["--setting-sources", "user"])
            .arg("--disable-slash-commands")
            .arg("--no-session-persistence");
        if let Some(model) = self.models.get(&job.role) {
            command.args(["--model", model]);
        }
        command
    }

    /// Writes the agent's settings file from scratch, so manual changes there
    /// never survive to the next run.
    fn prepare_config_dir(&self, job: &RoleJob) -> std::io::Result<()> {
        let dir = job.project_dir.join(CONFIG_DIR);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("settings.json"), "{}\n")
    }

    fn header(&self, job: &RoleJob) -> String {
        let model = self.models.get(&job.role).map_or("default", String::as_str);
        format!(
            "agent: claude, model: {model}, role: {:?}, round: {}\n",
            job.role, job.round
        )
    }
}

impl AgentRunner for ClaudeCode {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        let mut log = self.header(job);
        if let Err(e) = self.prepare_config_dir(job) {
            return failed(log, format!("cannot prepare {CONFIG_DIR}: {e}"));
        }

        let mut command = tokio::process::Command::from(self.command(job));
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // If we stop waiting (time-out), the agent is killed too.
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                return failed(log, format!("cannot start {}: {e}", self.program.display()));
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            // An agent may exit without reading everything; that is not our problem.
            let _ = stdin.write_all(job.prompt.as_bytes()).await;
        }

        let output = match tokio::time::timeout(self.timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return failed(log, format!("error while waiting for the agent: {e}"));
            }
            Err(_) => {
                let message = format!(
                    "timed out after {} s; the agent was stopped",
                    self.timeout.as_secs()
                );
                return failed(log, message);
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        log.push_str(&stdout);
        if !stderr.trim().is_empty() {
            log.push_str("\n--- stderr ---\n");
            log.push_str(&stderr);
        }

        let result = FinalResult::find(&stdout);
        let usage_limit_reached =
            looks_like_usage_limit(result.as_ref().map_or("", |r| &r.text), &stderr);
        let success = output.status.success()
            && !usage_limit_reached
            && result.as_ref().is_some_and(|r| !r.is_error);
        let message = match &result {
            _ if success => String::new(),
            Some(result) => result.text.clone(),
            None => format!("claude exited ({}) without a result", output.status),
        };
        AgentOutcome {
            success,
            usage_limit_reached,
            log,
            message,
        }
    }
}

/// Adds `message` to the log too, and returns a failed outcome.
fn failed(mut log: String, message: String) -> AgentOutcome {
    log.push_str(&message);
    log.push('\n');
    AgentOutcome {
        success: false,
        usage_limit_reached: false,
        log,
        message,
    }
}

/// The tools and permission rules one role gets.
#[derive(Debug, PartialEq, Eq)]
struct RoleRules {
    /// Which built-in tools exist for the agent at all.
    tools: Vec<&'static str>,
    /// What it may do without asking. With `dontAsk`, everything else is refused.
    allowed: Vec<String>,
    /// Refused even if something above allows it.
    denied: Vec<String>,
}

impl RoleRules {
    /// Mirrors `harness_core::permissions`. The harness checks the result with
    /// git anyway; these rules stop a wrong edit before it happens.
    fn for_job(job: &RoleJob) -> Self {
        let inbox = format!("Edit({}/**)", rule_path(&job.project_dir, &job.output_dir));
        let read = ["Read(./**)", "Glob", "Grep"].map(String::from);
        let (tools, allowed): (Vec<&str>, Vec<String>) = match job.role {
            Role::Architect => (
                vec!["Read", "Glob", "Grep", "Edit", "Write"],
                vec!["Edit(./docs/**)".into(), "Edit(./**/docs/**)".into(), inbox],
            ),
            Role::Developer => (
                vec!["Read", "Glob", "Grep", "Edit", "Write", "Bash"],
                vec!["Edit(./**)".into(), "Bash".into()],
            ),
            Role::Tester => (
                vec!["Read", "Glob", "Grep", "Edit", "Write", "Bash"],
                vec![
                    "Edit(./tests/**)".into(),
                    "Edit(./**/tests/**)".into(),
                    inbox,
                    "Bash".into(),
                ],
            ),
            Role::Security => (
                vec!["Read", "Glob", "Grep", "Write", "Bash"],
                vec![
                    inbox,
                    "Bash(cargo audit:*)".into(),
                    "Bash(cargo deny:*)".into(),
                ],
            ),
            // Lisa is never run as an agent.
            Role::Human => (vec![], vec![]),
        };
        RoleRules {
            tools,
            allowed: read.into_iter().chain(allowed).collect(),
            denied: [
                "Bash(git commit:*)",
                "Bash(git push:*)",
                "Edit(./.git/**)",
                "Edit(./.claude/**)",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

/// A path in Claude Code's rule syntax: `./x` inside the project, `//x` absolute.
fn rule_path(project_dir: &Path, path: &Path) -> String {
    match path.strip_prefix(project_dir) {
        Ok(relative) => format!("./{}", relative.display()),
        Err(_) => format!("/{}", path.display()),
    }
}

/// The last line of `stream-json` output: `{"type":"result","is_error":false,"result":"..."}`.
#[derive(Debug, PartialEq, Eq)]
struct FinalResult {
    is_error: bool,
    text: String,
}

impl FinalResult {
    fn find(stdout: &str) -> Option<Self> {
        stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .rfind(|value| value["type"] == "result")
            .map(|value| FinalResult {
                is_error: value["is_error"].as_bool().unwrap_or(true),
                text: value["result"].as_str().unwrap_or_default().to_string(),
            })
    }
}

/// Claude Code reports a used-up subscription only as text, so we look for the
/// usual phrases. Pausing by mistake is harmless: Lisa just runs again.
fn looks_like_usage_limit(result: &str, stderr: &str) -> bool {
    let text = format!("{result}\n{stderr}").to_lowercase();
    [
        "usage limit",
        "hit your limit",
        "limit reached",
        "rate limit",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(role: Role) -> RoleJob {
        RoleJob {
            task_id: "task-001".into(),
            round: 1,
            role,
            project_dir: PathBuf::from("/work/app"),
            prompt: "do it".into(),
            output_dir: PathBuf::from("/work/app/.harness/runs/task-001/inbox"),
        }
    }

    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn arg_after(command: &Command, flag: &str) -> String {
        let args = args(command);
        let i = args.iter().position(|a| a == flag).expect(flag);
        args[i + 1].clone()
    }

    #[test]
    fn the_architect_cannot_run_commands_and_writes_only_docs() {
        let agent = ClaudeCode::new(Secret::new("t"));
        let command = agent.command(&job(Role::Architect));

        assert_eq!(arg_after(&command, "--tools"), "Read,Glob,Grep,Edit,Write");
        assert_eq!(arg_after(&command, "--permission-mode"), "dontAsk");
        let allowed = arg_after(&command, "--allowedTools");
        assert!(allowed.contains("Edit(./docs/**)"), "{allowed}");
        assert!(allowed.contains("Edit(./.harness/runs/task-001/inbox/**)"));
        assert!(!allowed.contains("Bash"), "{allowed}");
    }

    #[test]
    fn security_may_only_run_checks() {
        let agent = ClaudeCode::new(Secret::new("t"));
        let allowed = arg_after(&agent.command(&job(Role::Security)), "--allowedTools");
        assert!(allowed.contains("Bash(cargo audit:*)"));
        assert!(!allowed.contains("Edit(./**)"));
        assert!(!allowed.contains(",Bash,"));
    }

    #[test]
    fn no_mcp_servers_and_no_personal_settings() {
        let command = ClaudeCode::new(Secret::new("t")).command(&job(Role::Developer));
        let args = args(&command);
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert_eq!(arg_after(&command, "--mcp-config"), r#"{"mcpServers":{}}"#);
        assert_eq!(arg_after(&command, "--setting-sources"), "user");
        assert!(args.contains(&"--disable-slash-commands".to_string()));
    }

    #[test]
    fn only_whitelisted_variables_and_the_token_reach_the_agent() {
        let command = ClaudeCode::new(Secret::new("my-token")).command(&job(Role::Developer));
        let envs: HashMap<String, Option<String>> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();

        assert_eq!(
            envs["CLAUDE_CONFIG_DIR"].as_deref(),
            Some("/work/app/.harness/agents/claude")
        );
        assert_eq!(envs["CLAUDE_CODE_OAUTH_TOKEN"].as_deref(), Some("my-token"));
        for name in envs.keys() {
            let ours = [
                "CLAUDE_CONFIG_DIR",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "DISABLE_AUTOUPDATER",
            ];
            assert!(
                INHERITED_ENV.contains(&name.as_str()) || ours.contains(&name.as_str()),
                "{name} should not be passed"
            );
        }
    }

    #[test]
    fn the_model_is_set_per_role() {
        let agent = ClaudeCode::new(Secret::new("t")).with_model(Role::Tester, "sonnet");
        assert_eq!(
            arg_after(&agent.command(&job(Role::Tester)), "--model"),
            "sonnet"
        );
        assert!(!args(&agent.command(&job(Role::Developer))).contains(&"--model".to_string()));
    }

    #[test]
    fn debug_output_hides_the_token() {
        let agent = ClaudeCode::new(Secret::new("my-token"));
        assert!(!format!("{agent:?}").contains("my-token"));
    }

    #[test]
    fn the_final_result_is_the_last_result_line() {
        let stdout = r#"{"type":"system","subtype":"init"}
{"type":"assistant","message":{}}
{"type":"result","is_error":false,"result":"Done."}"#;
        assert_eq!(
            FinalResult::find(stdout),
            Some(FinalResult {
                is_error: false,
                text: "Done.".into()
            })
        );
        assert_eq!(FinalResult::find("not json"), None);
    }

    #[test]
    fn usage_limit_phrases_are_recognised() {
        assert!(looks_like_usage_limit(
            "Claude AI usage limit reached|1759000000",
            ""
        ));
        assert!(looks_like_usage_limit(
            "",
            "You've hit your limit · resets 5pm"
        ));
        assert!(!looks_like_usage_limit("All tests pass.", ""));
    }
}
