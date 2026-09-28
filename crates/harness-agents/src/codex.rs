//! Runs a role with OpenAI Codex CLI (`codex exec`), isolated from Lisa's own setup.
//!
//! Isolation (docs/design.md, section 5.1):
//! - an empty environment plus a short whitelist (see `process`);
//! - `CODEX_HOME` points to `<project>/.harness/agents/codex/`, and
//!   `--ignore-user-config` / `--ignore-rules` skip every config and rules file,
//!   so the agent sees no MCP servers, plugins, hooks or profiles;
//! - login: `harness login codex` runs `codex login` into
//!   `~/.harness/credentials/codex/`. Only `auth.json` is copied into the
//!   project for the run and removed right after it;
//! - Codex's own sandbox (`workspace-write`): commands may write only inside
//!   the project. Codex has no per-folder rules like Claude Code, so the git
//!   check after the role is what enforces "Architect writes only docs/".
//!
//! DeepSeek (`agent = "codex+deepseek"`): the same Codex, with DeepSeek as its
//! model provider. DeepSeek speaks the Responses API that Codex uses. The API
//! key comes only from the `DEEPSEEK_API_KEY` variable in Lisa's shell and is
//! passed to this agent alone; it is never written to a file. No ChatGPT
//! login is copied in for these roles.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use harness_core::agent::{AgentOutcome, AgentRunner, RoleJob};
use harness_core::handoff::Role;

use crate::credentials::Secret;
use crate::process::{self, failed};

/// Where the agent's own settings live inside the project (ignored by git).
pub const CONFIG_DIR: &str = ".harness/agents/codex";
const AUTH_FILE: &str = "auth.json";

/// Codex features that reach outside the project or add tools we did not
/// choose. Written as `-c features.<name>=false`: an unknown name is only a
/// warning, so an older or newer Codex still starts.
const DISABLED_FEATURES: &[&str] = &[
    "apps",
    "plugins",
    "remote_plugin",
    "hooks",
    "browser_use",
    "browser_use_external",
    "computer_use",
    "in_app_browser",
    "image_generation",
    "skill_mcp_dependency_install",
    // Without this, a network problem makes Codex retry forever instead of
    // failing, and the role only ends at the time-out.
    "unbounded_connection_retries",
];

/// The variable in Lisa's shell that holds the DeepSeek API key.
pub const DEEPSEEK_KEY_ENV: &str = "DEEPSEEK_API_KEY";
/// DeepSeek's endpoint for Codex; Codex adds `responses` to it.
const DEEPSEEK_URL: &str = "https://api.deepseek.com/";
/// Used when harness.toml sets no model for the role.
pub const DEEPSEEK_DEFAULT_MODEL: &str = "deepseek-flash";

/// Who Codex talks to.
#[derive(Debug, Clone)]
enum Provider {
    /// OpenAI, with the ChatGPT login saved by `harness login codex`.
    ChatGpt,
    /// DeepSeek, with an API key.
    DeepSeek(Secret),
}

#[derive(Debug, Clone)]
pub struct Codex {
    program: PathBuf,
    provider: Provider,
    /// `~/.harness/credentials/codex`: holds the saved `auth.json`.
    auth_dir: PathBuf,
    models: HashMap<Role, String>,
    timeout: Duration,
}

impl Codex {
    pub fn new(auth_dir: impl Into<PathBuf>) -> Self {
        Self {
            program: PathBuf::from("codex"),
            provider: Provider::ChatGpt,
            auth_dir: auth_dir.into(),
            models: HashMap::new(),
            timeout: Duration::from_secs(30 * 60),
        }
    }

    /// Codex with DeepSeek models, paid by the API key.
    pub fn deepseek(api_key: Secret) -> Self {
        Self {
            provider: Provider::DeepSeek(api_key),
            ..Self::new(PathBuf::new())
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
        let mut command = process::base_command(&self.program, &job.project_dir);
        command
            .env("CODEX_HOME", job.project_dir.join(CONFIG_DIR))
            .arg("exec")
            // One JSON event per line; the prompt comes on standard input ("-").
            .arg("--json")
            .arg("--ephemeral")
            .args(["--ignore-user-config", "--ignore-rules"])
            .arg("--skip-git-repo-check")
            .args(["--cd".as_ref(), job.project_dir.as_os_str()])
            .args(["--sandbox", "workspace-write"])
            .args(["-c", "approval_policy=\"never\""])
            .args([
                "-c",
                &format!(
                    "sandbox_workspace_write.network_access={}",
                    needs_network(job.role)
                ),
            ]);
        for feature in DISABLED_FEATURES {
            command.args(["-c", &format!("features.{feature}=false")]);
        }
        if let Provider::DeepSeek(key) = &self.provider {
            command.env(DEEPSEEK_KEY_ENV, key.expose());
            for setting in [
                "model_provider=\"deepseek\"".to_string(),
                "model_providers.deepseek.name=\"DeepSeek\"".to_string(),
                format!("model_providers.deepseek.base_url=\"{DEEPSEEK_URL}\""),
                "model_providers.deepseek.wire_api=\"responses\"".to_string(),
                format!("model_providers.deepseek.env_key=\"{DEEPSEEK_KEY_ENV}\""),
                "forced_login_method=\"api\"".to_string(),
                // DeepSeek has no web search tool for Codex.
                "web_search=\"disabled\"".to_string(),
                // Commands the agent runs do not get variables named like
                // *KEY*, *SECRET* or *TOKEN*, so the key stays with Codex.
                "shell_environment_policy.ignore_default_excludes=false".to_string(),
            ] {
                command.args(["-c", &setting]);
            }
        }
        if let Some(model) = self.model(job.role) {
            command.args(["--model", model]);
        }
        command.arg("-");
        command
    }

    /// The role's model from harness.toml; DeepSeek needs one, so it has a default.
    fn model(&self, role: Role) -> Option<&str> {
        match (self.models.get(&role), &self.provider) {
            (Some(model), _) => Some(model),
            (None, Provider::DeepSeek(_)) => Some(DEEPSEEK_DEFAULT_MODEL),
            (None, Provider::ChatGpt) => None,
        }
    }

    /// Copies the saved login into the project's config folder for this run.
    fn put_auth(&self, job: &RoleJob) -> io::Result<()> {
        let dir = job.project_dir.join(CONFIG_DIR);
        fs::create_dir_all(&dir)?;
        let saved = fs::read(self.auth_dir.join(AUTH_FILE)).map_err(|e| {
            io::Error::new(
                e.kind(),
                "no Codex login saved; run `harness login codex` first",
            )
        })?;
        write_private(&dir.join(AUTH_FILE), &saved)
    }

    /// Codex may refresh its login during the run. Keep the newest one, then
    /// remove the copy so other roles' agents cannot read it.
    fn take_auth_back(&self, job: &RoleJob) {
        let copy = job.project_dir.join(CONFIG_DIR).join(AUTH_FILE);
        if let Ok(bytes) = fs::read(&copy) {
            let still_json = serde_json::from_slice::<serde_json::Value>(&bytes).is_ok();
            let saved = fs::read(self.auth_dir.join(AUTH_FILE)).ok();
            if still_json && saved.as_deref() != Some(bytes.as_slice()) {
                let _ = write_private(&self.auth_dir.join(AUTH_FILE), &bytes);
            }
        }
        let _ = fs::remove_file(copy);
    }

    fn header(&self, job: &RoleJob) -> String {
        let model = self.model(job.role).unwrap_or("default");
        let agent = match self.provider {
            Provider::ChatGpt => "codex",
            Provider::DeepSeek(_) => "codex+deepseek",
        };
        format!(
            "agent: {agent}, model: {model}, role: {:?}, round: {}\n",
            job.role, job.round
        )
    }
}

impl AgentRunner for Codex {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        let mut log = self.header(job);
        let chatgpt = matches!(self.provider, Provider::ChatGpt);
        let prepared = if chatgpt {
            self.put_auth(job)
        } else {
            fs::create_dir_all(job.project_dir.join(CONFIG_DIR))
        };
        if let Err(e) = prepared {
            return failed(log, format!("cannot prepare {CONFIG_DIR}: {e}"));
        }
        let result = process::run(self.command(job), &job.prompt, self.timeout).await;
        if chatgpt {
            self.take_auth_back(job);
        }
        let finished = match result {
            Ok(finished) => finished,
            Err(message) => return failed(log, message),
        };
        process::append_output(&mut log, &finished);

        let end = TurnEnd::find(&finished.stdout);
        let success = finished.status.success() && end == Some(TurnEnd::Completed);
        let message = match &end {
            _ if success => String::new(),
            Some(TurnEnd::Failed(message)) => message.clone(),
            _ => format!("codex exited ({}) without finishing", finished.status),
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

/// Roles that run commands such as `cargo test` or `cargo audit` may need to
/// download crates. The Architect only reads and writes documents.
fn needs_network(role: Role) -> bool {
    matches!(role, Role::Developer | Role::Tester | Role::Security)
}

/// How the turn ended, from `--json` output: `{"type":"turn.completed",...}` or
/// `{"type":"turn.failed","error":{"message":"..."}}`.
#[derive(Debug, PartialEq, Eq)]
enum TurnEnd {
    Completed,
    Failed(String),
}

impl TurnEnd {
    fn find(stdout: &str) -> Option<Self> {
        stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|event| match event["type"].as_str() {
                Some("turn.completed") => Some(TurnEnd::Completed),
                Some("turn.failed") => Some(TurnEnd::Failed(
                    event["error"]["message"]
                        .as_str()
                        .unwrap_or("unknown error")
                        .to_string(),
                )),
                _ => None,
            })
            .next_back()
    }
}

/// `auth.json` is a password too: readable only by its owner.
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

    #[test]
    fn codex_runs_isolated_in_the_project_sandbox() {
        let command = Codex::new("/creds/codex").command(&job(Role::Developer));
        let args = args(&command);
        for flag in [
            "exec",
            "--json",
            "--ignore-user-config",
            "--ignore-rules",
            "workspace-write",
            "approval_policy=\"never\"",
            "features.plugins=false",
            "features.hooks=false",
        ] {
            assert!(args.contains(&flag.to_string()), "missing {flag}: {args:?}");
        }
        assert_eq!(args.last().map(String::as_str), Some("-"));
        let home = command
            .get_envs()
            .find(|(k, _)| *k == "CODEX_HOME")
            .and_then(|(_, v)| v)
            .unwrap();
        assert_eq!(home, "/work/app/.harness/agents/codex");
    }

    #[test]
    fn only_command_running_roles_get_the_network() {
        let codex = Codex::new("/creds/codex");
        let architect = args(&codex.command(&job(Role::Architect)));
        assert!(architect.contains(&"sandbox_workspace_write.network_access=false".to_string()));
        let developer = args(&codex.command(&job(Role::Developer)));
        assert!(developer.contains(&"sandbox_workspace_write.network_access=true".to_string()));
    }

    #[test]
    fn no_api_keys_in_the_environment() {
        let command = Codex::new("/creds/codex").command(&job(Role::Tester));
        for (name, _) in command.get_envs() {
            let name = name.to_string_lossy();
            assert!(
                process::INHERITED_ENV.contains(&name.as_ref()) || name == "CODEX_HOME",
                "{name} should not be passed"
            );
        }
    }

    #[test]
    fn the_model_is_set_per_role() {
        let codex = Codex::new("/creds").with_model(Role::Tester, "gpt-5-codex");
        assert!(args(&codex.command(&job(Role::Tester))).contains(&"gpt-5-codex".to_string()));
        assert!(!args(&codex.command(&job(Role::Architect))).contains(&"--model".to_string()));
    }

    #[test]
    fn deepseek_gets_its_key_and_provider_but_no_chatgpt_login() {
        let codex = Codex::deepseek(Secret::new("sk-test"));
        let command = codex.command(&job(Role::Tester));
        let args = args(&command);
        for setting in [
            "model_provider=\"deepseek\"",
            "model_providers.deepseek.wire_api=\"responses\"",
            "model_providers.deepseek.env_key=\"DEEPSEEK_API_KEY\"",
            DEEPSEEK_DEFAULT_MODEL,
        ] {
            assert!(
                args.contains(&setting.to_string()),
                "missing {setting}: {args:?}"
            );
        }
        // The key is only in the environment, never in the arguments.
        assert!(!args.iter().any(|a| a.contains("sk-test")));
        let key = command
            .get_envs()
            .find(|(k, _)| *k == DEEPSEEK_KEY_ENV)
            .and_then(|(_, v)| v)
            .unwrap();
        assert_eq!(key, "sk-test");
        assert!(format!("{codex:?}").contains("Secret(***)"));
    }

    #[test]
    fn chatgpt_codex_gets_no_deepseek_key() {
        let command = Codex::new("/creds").command(&job(Role::Tester));
        assert!(!command.get_envs().any(|(k, _)| k == DEEPSEEK_KEY_ENV));
        assert!(!args(&command).contains(&"model_provider=\"deepseek\"".to_string()));
    }

    #[test]
    fn the_last_turn_event_decides() {
        let ok = "{\"type\":\"thread.started\"}\n{\"type\":\"turn.completed\",\"usage\":{}}";
        assert_eq!(TurnEnd::find(ok), Some(TurnEnd::Completed));
        let bad = "{\"type\":\"error\",\"message\":\"Reconnecting...\"}\n\
                   {\"type\":\"turn.failed\",\"error\":{\"message\":\"You've hit your usage limit\"}}";
        assert_eq!(
            TurnEnd::find(bad),
            Some(TurnEnd::Failed("You've hit your usage limit".into()))
        );
        assert_eq!(TurnEnd::find("WARNING: text\n"), None);
    }
}
