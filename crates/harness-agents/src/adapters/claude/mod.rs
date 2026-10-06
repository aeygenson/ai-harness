//! Runs a role with Claude Code (`claude -p`), isolated from Lisa's own setup.
//!
//! Isolation (docs/design.md, section 5.1):
//! - an empty environment plus a short whitelist, so API keys from the terminal
//!   never reach the agent;
//! - `CLAUDE_CONFIG_DIR` points to `<project>/.harness/agents/claude/`, so the
//!   agent does not see `~/.claude` (plugins, MCP servers, hooks, memory);
//! - login only through the token saved by the Agents tab («Sign in»); Claude
//!   Code removes the token from every command it runs (see [`HIDE_TOKEN`]);
//! - the role's tools and file rules are given as flags, and anything not
//!   allowed is refused without asking (`--permission-mode dontAsk`).

mod rules;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::install::credentials::Secret;
use crate::process::{self, failed};
use crate::role_settings::RoleSettings;
use harness_core::task::agent::{AgentOutcome, AgentRunner, RoleJob};
use harness_core::task::handoff::Role;
use rules::RoleRules;

/// Where the agent's own settings live inside the project (ignored by git).
pub const CONFIG_DIR: &str = ".harness/agents/claude";

/// Claude Code needs its token in its environment, but with this variable set
/// it removes the token from the environment of every command, hook and MCP
/// server it starts. On Linux it also runs the commands so they cannot read
/// other processes' environments. There it needs bubblewrap and does not
/// start without it, so the variable is set only when bubblewrap is there
/// ([`harness_platform::program::sandbox_ready`]).
const HIDE_TOKEN: &str = "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB";

/// The line the role's log gets when the token cannot be hidden.
const NO_SANDBOX_NOTE: &str = "note: bubblewrap (bwrap) is not installed, so the commands the \
agent runs can see its Claude login; install the package «bubblewrap» to hide it\n";

/// `--mcp-config` for a role without MCP servers.
const NO_MCP_SERVERS: &str = r#"{"mcpServers":{}}"#;

#[derive(Debug, Clone)]
pub struct ClaudeCode {
    program: PathBuf,
    token: Secret,
    /// Model, effort, MCP servers, plugins and time limit of each role.
    settings: RoleSettings,
}

impl ClaudeCode {
    /// Claude Code logged in with the saved `token`, running each role with its `settings`.
    pub fn new(token: Secret, settings: RoleSettings) -> Self {
        Self {
            program: PathBuf::from("claude"),
            token,
            settings,
        }
    }

    /// Which program to run instead of `claude` (tests use a fake script).
    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    /// Builds the full command for one job, without running it.
    /// A separate function, so tests can check every flag and variable.
    pub fn command(&self, job: &RoleJob) -> Command {
        self.command_with_mcp(job, NO_MCP_SERVERS.as_ref())
    }

    /// `mcp_config` is the JSON itself or a file with it: `--mcp-config` takes both.
    fn command_with_mcp(&self, job: &RoleJob, mcp_config: &std::ffi::OsStr) -> Command {
        let config_dir = job.project_dir.join(CONFIG_DIR);
        let plugins = self.settings.plugins(job.role);
        let rules = RoleRules::for_job(job, self.settings.servers(job.role), plugins);

        let mut command = process::base_command(&self.program, &job.project_dir);
        command
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
            // Only the role's own MCP servers, and settings only from our own config dir.
            .arg("--strict-mcp-config")
            .arg("--mcp-config")
            .arg(mcp_config)
            .args(["--setting-sources", "user"])
            .arg("--no-session-persistence");
        // Only the role's own plugins. A role without plugins gets no skills
        // of Claude Code's at all; the harness's own skills are in the prompt.
        for plugin in plugins {
            command.arg("--plugin-dir").arg(&plugin.path);
        }
        if plugins.is_empty() {
            command.arg("--disable-slash-commands");
        }
        if let Some(model) = self.settings.model(job.role) {
            command.args(["--model", model]);
        }
        if let Some(effort) = self.settings.effort(job.role) {
            command.args(["--effort", effort]);
        }
        if harness_platform::program::sandbox_ready() {
            command.env(HIDE_TOKEN, "1");
        }
        command
    }

    /// The role's servers as a temporary file outside the project, readable
    /// only by Lisa. Secrets are in it, so it is not passed as an argument;
    /// the file is deleted when the returned value is dropped.
    fn write_mcp_config(&self, role: Role) -> std::io::Result<tempfile::NamedTempFile> {
        let servers: serde_json::Map<String, serde_json::Value> = self
            .settings
            .servers(role)
            .iter()
            .map(|server| {
                let env: serde_json::Map<String, serde_json::Value> = server
                    .env
                    .iter()
                    .map(|(name, value)| (name.clone(), value.expose().into()))
                    .collect();
                let spec = serde_json::json!({
                    "type": "stdio",
                    "command": server.command,
                    "args": server.args,
                    "env": env,
                });
                (server.name.clone(), spec)
            })
            .collect();
        let mut file = tempfile::Builder::new()
            .prefix("harness-mcp-")
            .suffix(".json")
            .tempfile()?;
        let json = serde_json::json!({ "mcpServers": servers });
        std::io::Write::write_all(&mut file, json.to_string().as_bytes())?;
        Ok(file)
    }

    /// Writes the agent's settings file from scratch, so manual changes there
    /// never survive to the next run.
    fn prepare_config_dir(&self, job: &RoleJob) -> std::io::Result<()> {
        let dir = job.project_dir.join(CONFIG_DIR);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("settings.json"), self.settings(job.role))
    }

    /// Hooks run only if a plugin of the role is allowed to have them.
    fn settings(&self, role: Role) -> String {
        let hooks = self.settings.plugins(role).iter().any(|p| p.allow_hooks);
        format!("{}\n", serde_json::json!({ "disableAllHooks": !hooks }))
    }
}

impl AgentRunner for ClaudeCode {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        let mut log = self.settings.header("claude", job);
        if !harness_platform::program::sandbox_ready() {
            log.push_str(NO_SANDBOX_NOTE);
        }
        if let Err(e) = self.prepare_config_dir(job) {
            return failed(log, format!("cannot prepare {CONFIG_DIR}: {e}"));
        }

        let mcp_file = if self.settings.servers(job.role).is_empty() {
            None
        } else {
            match self.write_mcp_config(job.role) {
                Ok(file) => Some(file),
                Err(e) => return failed(log, format!("cannot write the MCP settings: {e}")),
            }
        };
        let command = match &mcp_file {
            Some(file) => self.command_with_mcp(job, file.path().as_os_str()),
            None => self.command(job),
        };
        let mut secrets = self.settings.server_secrets(job.role);
        secrets.push(self.token.expose());
        let result = process::run(command, &job.prompt, self.settings.timeout(), &secrets).await;
        let result = process::hide_secrets(result, secrets);
        let finished = match result {
            Ok(finished) => finished,
            Err(message) => return failed(log, message),
        };
        process::append_output(&mut log, &finished);
        let (stdout, stderr, status) = (&finished.stdout, &finished.stderr, finished.status);

        let result = FinalResult::find(stdout);
        let usage_limit_reached = process::looks_like_usage_limit(&format!(
            "{}\n{stderr}",
            result.as_ref().map_or("", |r| &r.text)
        ));
        let success = status.success()
            && !usage_limit_reached
            && result.as_ref().is_some_and(|r| !r.is_error);
        let message = match &result {
            _ if success => String::new(),
            Some(result) => result.text.clone(),
            None => format!("claude exited ({status}) without a result"),
        };
        AgentOutcome {
            success,
            usage_limit_reached,
            log,
            message,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::mcp::McpServer;
    use harness_core::plugins::Plugin;
    use std::collections::HashMap;
    use std::path::Path;

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
    fn model_and_effort_are_set_per_role() {
        let agent = ClaudeCode::new(
            Secret::new("t"),
            RoleSettings::default()
                .with_model(Role::Tester, "opus")
                .with_effort(Role::Tester, "max"),
        );
        let tester = args(&agent.command(&job(Role::Tester)));
        assert!(tester.windows(2).any(|w| w == ["--model", "opus"]));
        assert!(tester.windows(2).any(|w| w == ["--effort", "max"]));
        assert!(!args(&agent.command(&job(Role::Developer))).contains(&"--effort".to_string()));
    }

    fn arg_after(command: &Command, flag: &str) -> String {
        let args = args(command);
        let i = args.iter().position(|a| a == flag).expect(flag);
        args[i + 1].clone()
    }

    #[test]
    fn the_architect_cannot_run_commands_and_writes_only_docs() {
        let agent = ClaudeCode::new(Secret::new("t"), RoleSettings::default());
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
        let agent = ClaudeCode::new(Secret::new("t"), RoleSettings::default());
        let allowed = arg_after(&agent.command(&job(Role::Security)), "--allowedTools");
        assert!(allowed.contains("Bash(cargo audit:*)"));
        // Scanners of other ecosystems too, and git to read the change.
        assert!(allowed.contains("Bash(npm audit:*)"));
        assert!(allowed.contains("Bash(git diff:*)"));
        assert!(!allowed.contains("Edit(./**)"));
        assert!(!allowed.contains(",Bash,"));
    }

    #[test]
    fn the_tester_writes_tests_folders_and_test_files_next_to_the_code() {
        let agent = ClaudeCode::new(Secret::new("t"), RoleSettings::default());
        let allowed = arg_after(&agent.command(&job(Role::Tester)), "--allowedTools");
        for rule in [
            "Edit(./tests/**)",
            "Edit(./**/tests/**)",
            "Edit(./*_test.*)",
            "Edit(./**/*_test.*)",
            "Edit(./**/*.spec.*)",
        ] {
            assert!(allowed.contains(rule), "{rule} in {allowed}");
        }
        assert!(!allowed.contains("Edit(./**)"), "{allowed}");
    }

    #[test]
    fn no_mcp_servers_and_no_personal_settings() {
        let command = ClaudeCode::new(Secret::new("t"), RoleSettings::default())
            .command(&job(Role::Developer));
        let args = args(&command);
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert_eq!(arg_after(&command, "--mcp-config"), r#"{"mcpServers":{}}"#);
        assert_eq!(arg_after(&command, "--setting-sources"), "user");
        assert!(args.contains(&"--disable-slash-commands".to_string()));
    }

    #[test]
    fn a_role_gets_only_its_plugins_and_hooks_stay_off_unless_allowed() {
        let plugin = |name: &str, allow_hooks| Plugin {
            name: name.into(),
            path: PathBuf::from(format!("/work/app/.harness/plugins/{name}")),
            allow_hooks,
        };
        let agent = ClaudeCode::new(
            Secret::new("t"),
            RoleSettings::default()
                .with_plugins(Role::Security, vec![plugin("review", false)])
                .with_plugins(Role::Developer, vec![plugin("fmt", true)]),
        );

        let security = agent.command(&job(Role::Security));
        assert_eq!(
            arg_after(&security, "--plugin-dir"),
            "/work/app/.harness/plugins/review"
        );
        let security_args = args(&security);
        // Plugin skills need skills switched on and the Skill tool.
        assert!(!security_args.contains(&"--disable-slash-commands".to_string()));
        assert!(arg_after(&security, "--tools")
            .split(',')
            .any(|t| t == "Skill"));
        assert!(arg_after(&security, "--allowedTools").contains("mcp__plugin_review_*"));
        assert_eq!(
            agent.settings(Role::Security),
            "{\"disableAllHooks\":true}\n"
        );
        assert_eq!(
            agent.settings(Role::Developer),
            "{\"disableAllHooks\":false}\n"
        );

        let tester = args(&agent.command(&job(Role::Tester)));
        assert!(!tester.contains(&"--plugin-dir".to_string()));
        assert!(tester.contains(&"--disable-slash-commands".to_string()));
        assert_eq!(agent.settings(Role::Tester), "{\"disableAllHooks\":true}\n");
    }

    #[test]
    fn a_role_gets_only_its_mcp_servers_from_a_private_file() {
        use std::collections::BTreeMap;

        let server = McpServer {
            name: "context7".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@upstash/context7-mcp".into()],
            env: BTreeMap::from([("CONTEXT7_API_KEY".into(), Secret::new("ctx-secret"))]),
        };
        let agent = ClaudeCode::new(
            Secret::new("t"),
            RoleSettings::default().with_mcp_servers(Role::Developer, vec![server]),
        );

        let rules = RoleRules::for_job(
            &job(Role::Developer),
            agent.settings.servers(Role::Developer),
            &[],
        );
        assert!(rules.allowed.contains(&"mcp__context7".to_string()));
        let tester = RoleRules::for_job(
            &job(Role::Tester),
            agent.settings.servers(Role::Tester),
            &[],
        );
        assert!(!tester.allowed.iter().any(|rule| rule.starts_with("mcp__")));

        let file = agent.write_mcp_config(Role::Developer).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(file.path()).unwrap()).unwrap();
        let spec = &json["mcpServers"]["context7"];
        assert_eq!(spec["command"], "npx");
        assert_eq!(spec["args"][1], "@upstash/context7-mcp");
        assert_eq!(spec["env"]["CONTEXT7_API_KEY"], "ctx-secret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(file.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // The file, not the secret, is what the command line carries.
        let command = agent.command_with_mcp(&job(Role::Developer), file.path().as_os_str());
        assert_eq!(
            arg_after(&command, "--mcp-config"),
            file.path().to_string_lossy()
        );
        assert!(!args(&command).iter().any(|a| a.contains("ctx-secret")));
    }

    #[test]
    fn only_whitelisted_variables_and_the_token_reach_the_agent() {
        let command = ClaudeCode::new(Secret::new("my-token"), RoleSettings::default())
            .command(&job(Role::Developer));
        let envs: HashMap<String, Option<String>> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();

        // Joined by the system's own separator (`\\` on Windows).
        let config_dir = Path::new("/work/app").join(".harness/agents/claude");
        assert_eq!(
            envs["CLAUDE_CONFIG_DIR"].as_deref(),
            Some(config_dir.to_string_lossy().as_ref())
        );
        assert_eq!(envs["CLAUDE_CODE_OAUTH_TOKEN"].as_deref(), Some("my-token"));
        for name in envs.keys() {
            let ours = [
                "CLAUDE_CONFIG_DIR",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "DISABLE_AUTOUPDATER",
                HIDE_TOKEN,
            ];
            assert!(
                harness_platform::env::is_inherited(name) || ours.contains(&name.as_str()),
                "{name} should not be passed"
            );
        }
    }

    #[test]
    fn the_token_is_hidden_from_the_agents_commands_where_possible() {
        let command = ClaudeCode::new(Secret::new("t"), RoleSettings::default())
            .command(&job(Role::Developer));
        let hide = command
            .get_envs()
            .find(|(name, _)| *name == HIDE_TOKEN)
            .and_then(|(_, value)| value);
        if harness_platform::program::sandbox_ready() {
            assert_eq!(hide, Some(std::ffi::OsStr::new("1")));
        } else {
            assert_eq!(hide, None);
        }
    }

    #[test]
    fn the_model_is_set_per_role() {
        let agent = ClaudeCode::new(
            Secret::new("t"),
            RoleSettings::default().with_model(Role::Tester, "sonnet"),
        );
        assert_eq!(
            arg_after(&agent.command(&job(Role::Tester)), "--model"),
            "sonnet"
        );
        assert!(!args(&agent.command(&job(Role::Developer))).contains(&"--model".to_string()));
    }

    #[test]
    fn debug_output_hides_the_token() {
        let agent = ClaudeCode::new(Secret::new("my-token"), RoleSettings::default());
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
}
