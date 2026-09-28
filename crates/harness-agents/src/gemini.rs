//! Runs a role with Google Gemini CLI (`gemini -p`), isolated from Lisa's own setup.
//!
//! Isolation (docs/design.md, section 5.4):
//! - an empty environment plus a short whitelist (see `process`), so no
//!   `GEMINI_API_KEY` or `GOOGLE_API_KEY` reaches the agent;
//! - `GEMINI_CLI_HOME` points to `<project>/.harness/agents/gemini/`, so Gemini
//!   reads its settings from `<that>/.gemini/` and never from `~/.gemini`;
//! - `-e none` turns every extension off and `--allowed-mcp-server-names`
//!   with a name nobody uses turns every MCP server off;
//! - login: `harness login gemini` logs in with Google into
//!   `~/.harness/credentials/gemini/`. Only `oauth_creds.json` is copied into
//!   the project for the run and removed right after it;
//! - Gemini has no sandbox without Docker, so the role's limits are a policy
//!   file (`--policy`): our rules are in the "user" tier and beat the "allow
//!   everything" rule of yolo mode. The git check after the role still
//!   enforces which folders the role may change.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use harness_core::agent::{AgentOutcome, AgentRunner, RoleJob};
use harness_core::handoff::Role;

use crate::process::{self, failed};

/// Where the agent's own settings live inside the project (ignored by git).
pub const CONFIG_DIR: &str = ".harness/agents/gemini";
/// Gemini keeps its files in `$GEMINI_CLI_HOME/.gemini/`.
const GEMINI_DIR: &str = ".gemini";
const AUTH_FILE: &str = "oauth_creds.json";
const POLICY_FILE: &str = "policy.toml";
/// An MCP server name nobody uses: with it only "that" server is allowed,
/// so in fact none is.
const NO_MCP_SERVERS: &str = "harness-allows-no-mcp-servers";

#[derive(Debug, Clone)]
pub struct Gemini {
    program: PathBuf,
    /// `~/.harness/credentials/gemini`: holds the saved `.gemini/oauth_creds.json`.
    auth_dir: PathBuf,
    models: HashMap<Role, String>,
    timeout: Duration,
}

impl Gemini {
    pub fn new(auth_dir: impl Into<PathBuf>) -> Self {
        Self {
            program: PathBuf::from("gemini"),
            auth_dir: auth_dir.into(),
            models: HashMap::new(),
            timeout: Duration::from_secs(30 * 60),
        }
    }

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
    pub fn command(&self, job: &RoleJob) -> Command {
        let home = job.project_dir.join(CONFIG_DIR);
        let mut command = process::base_command(&self.program, &job.project_dir);
        command
            .env("GEMINI_CLI_HOME", &home)
            // "Login with Google", the subscription, never an API key.
            .env("GOOGLE_GENAI_USE_GCA", "true")
            // Keep the login in the file we copy in, not in the system keychain.
            .env("GEMINI_FORCE_FILE_STORAGE", "true")
            // The project folder is trusted; otherwise yolo mode is refused.
            .env("GEMINI_CLI_TRUST_WORKSPACE", "true")
            // The prompt comes on standard input; `-p` is added after it.
            .args(["-p", "Do the task described above."])
            .args(["--output-format", "stream-json"])
            .args(["--approval-mode", "yolo"])
            .arg("--skip-trust")
            .args(["-e", "none"])
            .args(["--allowed-mcp-server-names", NO_MCP_SERVERS])
            .args(["--policy".as_ref(), home.join(POLICY_FILE).as_os_str()]);
        if let Some(model) = self.models.get(&job.role) {
            command.args(["--model", model]);
        }
        command
    }

    /// Writes the role's policy and copies the saved login in for this run.
    fn prepare(&self, job: &RoleJob) -> io::Result<()> {
        let home = job.project_dir.join(CONFIG_DIR);
        fs::create_dir_all(home.join(GEMINI_DIR))?;
        fs::write(home.join(POLICY_FILE), policy(job.role))?;
        let saved = fs::read(self.saved_auth()).map_err(|e| {
            io::Error::new(
                e.kind(),
                "no Gemini login saved; run `harness login gemini` first",
            )
        })?;
        write_private(&home.join(GEMINI_DIR).join(AUTH_FILE), &saved)
    }

    fn saved_auth(&self) -> PathBuf {
        self.auth_dir.join(GEMINI_DIR).join(AUTH_FILE)
    }

    /// Gemini refreshes its login during the run. Keep the newest one, then
    /// remove the copy so other roles' agents cannot read it.
    fn take_auth_back(&self, job: &RoleJob) {
        let copy = job
            .project_dir
            .join(CONFIG_DIR)
            .join(GEMINI_DIR)
            .join(AUTH_FILE);
        if let Ok(bytes) = fs::read(&copy) {
            let still_json = serde_json::from_slice::<serde_json::Value>(&bytes).is_ok();
            let saved = fs::read(self.saved_auth()).ok();
            if still_json && saved.as_deref() != Some(bytes.as_slice()) {
                let _ = write_private(&self.saved_auth(), &bytes);
            }
        }
        let _ = fs::remove_file(copy);
    }

    fn header(&self, job: &RoleJob) -> String {
        let model = self.models.get(&job.role).map_or("default", String::as_str);
        format!(
            "agent: gemini, model: {model}, role: {:?}, round: {}\n",
            job.role, job.round
        )
    }
}

impl AgentRunner for Gemini {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        let mut log = self.header(job);
        if let Err(e) = self.prepare(job) {
            return failed(log, format!("cannot prepare {CONFIG_DIR}: {e}"));
        }
        let result = process::run(self.command(job), &job.prompt, self.timeout).await;
        self.take_auth_back(job);
        let finished = match result {
            Ok(finished) => finished,
            Err(message) => return failed(log, message),
        };
        process::append_output(&mut log, &finished);

        let end = RunResult::find(&finished.stdout);
        let success = finished.status.success() && end == Some(RunResult::Success);
        let message = match &end {
            _ if success => String::new(),
            Some(RunResult::Error(message)) => message.clone(),
            _ => format!(
                "gemini exited ({}) without finishing: {}",
                finished.status,
                last_line(&finished.stderr)
            ),
        };
        let usage_limit_reached =
            !success && process::looks_like_usage_limit(&format!("{message}\n{}", finished.stderr));
        AgentOutcome {
            success: success && !usage_limit_reached,
            usage_limit_reached,
            log,
            message,
        }
    }
}

/// The role's rules for Gemini's policy engine. A higher `priority` wins, so
/// the few things a role may do (priority 800) beat its wider bans (100),
/// and the bans for every role (900) beat everything.
fn policy(role: Role) -> String {
    let mut rules =
        String::from("# Written by the harness before each run; changes here are overwritten.\n\n");
    let mut rule = |tool: &str, extra: &str, decision: &str, priority: u32, why: &str| {
        rules.push_str(&format!(
            "[[rule]]\ntoolName = \"{tool}\"\n{extra}decision = \"{decision}\"\n\
             priority = {priority}\n"
        ));
        if !why.is_empty() {
            rules.push_str(&format!("denyMessage = \"{why}\"\n"));
        }
        rules.push('\n');
    };
    rule(
        "run_shell_command",
        "commandPrefix = [\"git commit\", \"git push\"]\n",
        "deny",
        900,
        "The harness makes the commits; do not commit or push.",
    );
    for tool in ["google_web_search", "web_fetch"] {
        rule(
            tool,
            "",
            "deny",
            900,
            "Work only with the files of the project.",
        );
    }
    match role {
        Role::Architect => rule(
            "run_shell_command",
            "",
            "deny",
            100,
            "The Architect only reads and writes documents.",
        ),
        Role::Security => {
            for tool in ["write_file", "replace"] {
                rule(
                    tool,
                    "",
                    "deny",
                    100,
                    "Security only reviews and does not change files.",
                );
            }
            rule(
                "run_shell_command",
                "commandPrefix = [\"cargo audit\", \"cargo deny\"]\n",
                "allow",
                800,
                "",
            );
            rule(
                "run_shell_command",
                "",
                "deny",
                100,
                "Security may run only `cargo audit` and `cargo deny`.",
            );
        }
        Role::Developer | Role::Tester | Role::Human => {}
    }
    rules
}

/// How the run ended, from `stream-json` output: the last event
/// `{"type":"result","status":"success"}` or
/// `{"type":"result","status":"error","error":{"message":"..."}}`.
#[derive(Debug, PartialEq, Eq)]
enum RunResult {
    Success,
    Error(String),
}

impl RunResult {
    fn find(stdout: &str) -> Option<Self> {
        stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| event["type"] == "result")
            .map(|event| match event["status"].as_str() {
                Some("success") => RunResult::Success,
                _ => RunResult::Error(
                    event["error"]["message"]
                        .as_str()
                        .unwrap_or("unknown error")
                        .to_string(),
                ),
            })
            .next_back()
    }
}

fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
}

/// `oauth_creds.json` is a password too: readable only by its owner.
#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)
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

    fn env<'a>(command: &'a Command, name: &str) -> Option<&'a std::ffi::OsStr> {
        command
            .get_envs()
            .find(|(k, _)| *k == name)
            .and_then(|(_, v)| v)
    }

    #[test]
    fn gemini_runs_isolated_without_extensions_or_mcp() {
        let command = Gemini::new("/creds/gemini").command(&job(Role::Developer));
        let args = args(&command);
        for flag in [
            "stream-json",
            "yolo",
            "--skip-trust",
            "none",
            NO_MCP_SERVERS,
            "/work/app/.harness/agents/gemini/policy.toml",
        ] {
            assert!(args.contains(&flag.to_string()), "missing {flag}: {args:?}");
        }
        assert_eq!(
            env(&command, "GEMINI_CLI_HOME").unwrap(),
            "/work/app/.harness/agents/gemini"
        );
        assert_eq!(env(&command, "GOOGLE_GENAI_USE_GCA").unwrap(), "true");
    }

    #[test]
    fn no_api_keys_in_the_environment() {
        let command = Gemini::new("/creds/gemini").command(&job(Role::Tester));
        let ours = [
            "GEMINI_CLI_HOME",
            "GOOGLE_GENAI_USE_GCA",
            "GEMINI_FORCE_FILE_STORAGE",
            "GEMINI_CLI_TRUST_WORKSPACE",
        ];
        for (name, _) in command.get_envs() {
            let name = name.to_string_lossy();
            assert!(
                process::INHERITED_ENV.contains(&name.as_ref()) || ours.contains(&name.as_ref()),
                "{name} should not be passed"
            );
        }
    }

    #[test]
    fn the_model_is_set_per_role() {
        let gemini = Gemini::new("/creds").with_model(Role::Tester, "gemini-2.5-pro");
        assert!(args(&gemini.command(&job(Role::Tester))).contains(&"gemini-2.5-pro".to_string()));
        assert!(!args(&gemini.command(&job(Role::Architect))).contains(&"--model".to_string()));
    }

    #[test]
    fn every_policy_is_valid_toml_and_bans_commits() {
        for role in [
            Role::Architect,
            Role::Developer,
            Role::Tester,
            Role::Security,
        ] {
            let text = policy(role);
            let parsed: toml::Value = toml::from_str(&text).unwrap();
            let rules = parsed["rule"].as_array().unwrap();
            assert!(
                rules.iter().any(|r| r["decision"].as_str() == Some("deny")
                    && r.get("commandPrefix").is_some_and(|p| p
                        .as_array()
                        .unwrap()
                        .contains(&toml::Value::from("git commit")))),
                "{role:?}: {text}"
            );
        }
    }

    #[test]
    fn each_role_gets_its_own_limits() {
        assert!(policy(Role::Architect).contains("The Architect only reads"));
        assert!(!policy(Role::Developer).contains("priority = 100"));
        let security = policy(Role::Security);
        assert!(security.contains("toolName = \"write_file\""));
        assert!(security.contains("commandPrefix = [\"cargo audit\", \"cargo deny\"]"));
    }

    #[test]
    fn the_last_result_event_decides() {
        let ok = "{\"type\":\"init\"}\n{\"type\":\"result\",\"status\":\"success\",\"stats\":{}}";
        assert_eq!(RunResult::find(ok), Some(RunResult::Success));
        let bad = "{\"type\":\"init\"}\n{\"type\":\"result\",\"status\":\"error\",\
                   \"error\":{\"type\":\"Quota\",\"message\":\"Rate limit exceeded\"}}";
        assert_eq!(
            RunResult::find(bad),
            Some(RunResult::Error("Rate limit exceeded".into()))
        );
        assert_eq!(RunResult::find("YOLO mode is enabled.\n"), None);
    }
}
