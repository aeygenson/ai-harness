//! Runs a role with DeepSeek Harness (`dsh --profile headless`), DeepSeek's
//! own console agent, isolated from Lisa's own setup.
//!
//! Isolation:
//! - an empty environment plus a short whitelist (see `process`), so the
//!   DeepSeek key is not in the environment the agent's commands inherit;
//! - `DSH_HOME` (dsh's settings, sessions and keys) is a fresh temporary
//!   folder outside the project, deleted after the role. It holds only the
//!   key, in dsh's own `.credentials.yaml`, which dsh never puts into the
//!   environment, and our patch file. So Lisa's own dsh settings, providers,
//!   skills and MCP servers never reach a role, and no history is kept;
//! - the patch (`--patch`, dsh's settings overlay) sets the model and effort,
//!   gives the role its own MCP servers from harness.toml and turns off what
//!   should not run: uploading the session log and feedback to DeepSeek, and
//!   skills from folders (the harness gives skills through the prompt);
//! - dsh's own sandbox (`workspace-write`, bwrap or Landlock on Linux):
//!   commands may write only inside the project. A command that needs more
//!   would ask, and headless mode cannot ask, so it is refused and the run
//!   goes on. As with Codex, the git check after the role is what enforces
//!   which folders a role may change.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use harness_core::config::AgentKind;
use harness_core::task::agent::{AgentOutcome, AgentRunner, RoleJob};
use harness_core::task::handoff::Role;
use serde_json::{json, Value};

use crate::install::credentials::Secret;
use crate::process::{self, failed};
use crate::role_settings::RoleSettings;

/// The variable in Lisa's shell that may hold the DeepSeek API key; it wins
/// over the saved one. dsh's own name for the key too.
pub const KEY_ENV: &str = "DEEPSEEK_API_KEY";
/// The model dsh uses when harness.toml names none.
pub const DEFAULT_MODEL: &str = "deepseek-flash";
/// The effort levels of DeepSeek models in dsh, weakest first.
pub const EFFORTS: [&str; 4] = ["off", "low", "high", "max"];
/// The level dsh uses when harness.toml names none.
pub const DEFAULT_EFFORT: &str = "high";

/// dsh's file for keys inside `DSH_HOME`.
const CREDENTIALS_FILE: &str = ".credentials.yaml";
/// Our settings overlay inside `DSH_HOME`.
const PATCH_FILE: &str = "harness.patch.json";
/// dsh's provider for DeepSeek's own API.
const PROVIDER: &str = "deepseek-official";

#[derive(Debug, Clone)]
pub struct Dsh {
    program: PathBuf,
    key: Secret,
    /// Model, effort, MCP servers and time limit of each role (plugins are not used).
    settings: RoleSettings,
}

impl Dsh {
    /// dsh with the DeepSeek `key`, running each role with its `settings`.
    pub fn new(key: Secret, settings: RoleSettings) -> Self {
        Self {
            program: PathBuf::from("dsh"),
            key,
            settings,
        }
    }

    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    /// Builds the full command for one job, without running it. `home` is the
    /// temporary `DSH_HOME` prepared by `prepare_home`. The prompt goes in on
    /// standard input (`-`).
    pub fn command(&self, job: &RoleJob, home: &Path) -> Command {
        let mut command = process::base_command(&self.program, &job.project_dir);
        command
            .env("DSH_HOME", home)
            // Shared skills of all agents (`~/.agents`) stay out too.
            .env("DSH_AGENTS_HOME", home.join("agents"))
            .env("DSH_PERMISSION_MODE", "workspace-write")
            .env("DSH_TELEMETRY_MODE", "DISABLED")
            .args(["--profile", "headless", "--patch"])
            .arg(home.join(PATCH_FILE))
            .args(["--json", "-"]);
        command
    }

    /// Makes a temporary `DSH_HOME` with the key and the role's patch. It is
    /// deleted when the returned value is dropped.
    fn prepare_home(&self, role: Role) -> io::Result<tempfile::TempDir> {
        let home = tempfile::Builder::new().prefix("harness-dsh-").tempdir()?;
        // dsh refuses a key file other users can read; the patch may hold
        // MCP server secrets, so it is private too.
        let credentials = json!({
            "version": 1,
            "refs": { KEY_ENV: self.key.expose() },
        });
        crate::install::credentials::write_private(
            &home.path().join(CREDENTIALS_FILE),
            &credentials.to_string(),
        )?;
        crate::install::credentials::write_private(
            &home.path().join(PATCH_FILE),
            &self.patch(role, home.path()),
        )?;
        fs::create_dir_all(home.path().join("agents"))?;
        Ok(home)
    }

    /// dsh's settings overlay for `role`: a list of changes to its plugins,
    /// in JSON (which dsh reads as YAML). `home` is the temporary `DSH_HOME`.
    fn patch(&self, role: Role, home: &Path) -> String {
        let model = self.settings.model(role).unwrap_or(DEFAULT_MODEL);
        let effort = self.settings.effort(role).unwrap_or(DEFAULT_EFFORT);
        let mut patch = vec![
            json!({
                "id": "agent-default-model",
                "config": { "provider": PROVIDER, "model": model, "reasoningEffort": effort },
            }),
            // On by default: every session log goes to DeepSeek with the requests.
            json!({ "id": "session-log-deepseek", "disabled": true }),
            json!({ "id": "session-telemetry-otel", "config": { "mode": "DISABLED" } }),
            // Skills come in the prompt; none from the project or Lisa's folders.
            json!({ "id": "skill-filesystem", "config": { "includeDefaultRoots": false } }),
            // Long command output is kept in files; by default in a new
            // `/tmp/dsh-spill-*` folder that stays after the run.
            json!({
                "id": "spill-local",
                "config": { "root": home.join("spill"), "cleanupPeriodDays": 0 },
            }),
        ];
        let servers: Vec<Value> = self
            .settings
            .servers(role)
            .iter()
            .map(|server| {
                let env: serde_json::Map<String, Value> = server
                    .env
                    .iter()
                    .map(|(name, value)| (name.clone(), value.expose().into()))
                    .collect();
                json!({
                    "id": format!("harness-mcp-{}", server.name),
                    "name": "@deepseek-ai/dsh-mcp-client",
                    "config": {
                        "serverName": server.name,
                        "transport": "stdio",
                        "command": server.command,
                        "args": server.args,
                        "env": env,
                    },
                })
            })
            .collect();
        if !servers.is_empty() {
            patch.push(json!({ "insert": servers }));
        }
        serde_json::to_string_pretty(&patch).expect("the patch is plain JSON")
    }
}

impl AgentRunner for Dsh {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        let log = self.settings.header(AgentKind::Dsh, job);
        let home = match self.prepare_home(job.role) {
            Ok(home) => home,
            Err(e) => return failed(log, format!("cannot prepare dsh's home: {e}")),
        };
        let mut secrets = self.settings.server_secrets(job.role);
        secrets.push(self.key.expose());
        let result = process::run(
            self.command(job, home.path()),
            &job.prompt,
            self.settings.timeout(),
            &secrets,
        )
        .await;
        drop(home);
        let result = process::hide_secrets(result, secrets);
        outcome(log, result)
    }
}

/// Turns what dsh printed into the outcome of the role. Exit code 0 means the
/// task was completed; otherwise the reason is in the last `turn_end` event or
/// in a `dsh: <code>: <message>` line on stderr.
fn outcome(mut log: String, result: Result<process::Finished, String>) -> AgentOutcome {
    let finished = match result {
        Ok(finished) => finished,
        Err(message) => return failed(log, message),
    };
    process::append_output(&mut log, &finished);
    let success = finished.status.success();
    let message = if success {
        String::new()
    } else {
        turn_error(&finished.stdout)
            .or_else(|| dsh_error(&finished.stderr))
            .unwrap_or_else(|| format!("dsh exited ({}) without finishing", finished.status))
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

/// The error of the last `{"type":"status","phase":"turn_end","reason":{"kind":"error",...}}`.
fn turn_error(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .rfind(|event| event["type"] == "status" && event["phase"] == "turn_end")
        .and_then(|event| {
            let error = &event["reason"]["error"];
            let message = error["message"].as_str()?;
            Some(match error["code"].as_str() {
                Some(code) => format!("dsh: {code}: {message}"),
                None => format!("dsh: {message}"),
            })
        })
}

/// The last `dsh: ...` line dsh printed on stderr.
fn dsh_error(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("dsh:"))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::mcp::McpServer;

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
    fn dsh_runs_headless_with_its_own_home_and_the_prompt_on_stdin() {
        let dsh = Dsh::new(Secret::new("sk-secret"), RoleSettings::default());
        let command = dsh.command(&job(Role::Developer), Path::new("/tmp/home"));
        let args = args(&command);
        assert_eq!(&args[..3], ["--profile", "headless", "--patch"]);
        assert!(args[3].ends_with(PATCH_FILE), "{args:?}");
        assert_eq!(&args[4..], ["--json", "-"]);
        assert!(!args.iter().any(|a| a.contains("do it")), "{args:?}");
        assert_eq!(env(&command, "DSH_HOME").unwrap(), "/tmp/home");
        assert_eq!(
            env(&command, "DSH_PERMISSION_MODE").unwrap(),
            "workspace-write"
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/work/app")));
    }

    #[test]
    fn the_key_is_never_in_the_environment() {
        let command = Dsh::new(Secret::new("sk-secret"), RoleSettings::default())
            .command(&job(Role::Tester), Path::new("/h"));
        for (name, value) in command.get_envs() {
            let name = name.to_string_lossy();
            assert!(
                harness_platform::env::is_inherited(&name) || name.starts_with("DSH_"),
                "{name} should not be passed"
            );
            assert!(!value
                .unwrap_or_default()
                .to_string_lossy()
                .contains("sk-secret"));
        }
    }

    #[test]
    fn the_home_holds_the_key_privately_and_is_deleted() {
        let dsh = Dsh::new(Secret::new("sk-secret"), RoleSettings::default());
        let home = dsh.prepare_home(Role::Architect).unwrap();
        let credentials = home.path().join(CREDENTIALS_FILE);
        let parsed: Value =
            serde_json::from_str(&fs::read_to_string(&credentials).unwrap()).unwrap();
        assert_eq!(parsed["refs"]["DEEPSEEK_API_KEY"], "sk-secret");
        assert_eq!(parsed["version"], 1);
        assert!(harness_platform::private::is_private(&credentials).unwrap());
        assert!(home.path().join(PATCH_FILE).is_file());
        let path = home.path().to_path_buf();
        drop(home);
        assert!(!path.exists());
    }

    #[test]
    fn the_patch_sets_model_and_effort_and_stops_uploads() {
        let dsh = Dsh::new(
            Secret::new("k"),
            RoleSettings::default()
                .with_model(Role::Developer, "deepseek-v4-pro")
                .with_effort(Role::Developer, "max"),
        );
        let patch: Value =
            serde_json::from_str(&dsh.patch(Role::Developer, Path::new("/h"))).unwrap();
        let entry = |id: &str| {
            patch
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == id)
                .cloned()
                .unwrap()
        };
        let model = entry("agent-default-model");
        assert_eq!(model["config"]["model"], "deepseek-v4-pro");
        assert_eq!(model["config"]["reasoningEffort"], "max");
        assert_eq!(model["config"]["provider"], "deepseek-official");
        assert_eq!(entry("session-log-deepseek")["disabled"], true);
        assert_eq!(
            entry("session-telemetry-otel")["config"]["mode"],
            "DISABLED"
        );
        assert_eq!(
            entry("skill-filesystem")["config"]["includeDefaultRoots"],
            false
        );
        assert_eq!(
            entry("spill-local")["config"]["root"],
            Path::new("/h").join("spill").to_string_lossy().as_ref()
        );

        // Another role keeps dsh's defaults.
        let tester: Value =
            serde_json::from_str(&dsh.patch(Role::Tester, Path::new("/h"))).unwrap();
        assert_eq!(tester[0]["config"]["model"], DEFAULT_MODEL);
        assert_eq!(tester[0]["config"]["reasoningEffort"], DEFAULT_EFFORT);
    }

    #[test]
    fn only_the_roles_own_mcp_servers_are_inserted() {
        use std::collections::BTreeMap;

        let server = McpServer {
            name: "context7".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@upstash/context7-mcp".into()],
            env: BTreeMap::from([("CONTEXT7_API_KEY".into(), Secret::new("ctx-secret"))]),
        };
        let dsh = Dsh::new(
            Secret::new("k"),
            RoleSettings::default().with_mcp_servers(Role::Security, vec![server]),
        );
        let patch: Value =
            serde_json::from_str(&dsh.patch(Role::Security, Path::new("/h"))).unwrap();
        let insert = patch
            .as_array()
            .unwrap()
            .iter()
            .find_map(|e| e.get("insert"))
            .unwrap();
        let config = &insert[0]["config"];
        assert_eq!(insert[0]["name"], "@deepseek-ai/dsh-mcp-client");
        assert_eq!(config["serverName"], "context7");
        assert_eq!(config["transport"], "stdio");
        assert_eq!(config["args"][1], "@upstash/context7-mcp");
        assert_eq!(config["env"]["CONTEXT7_API_KEY"], "ctx-secret");

        let tester: Value =
            serde_json::from_str(&dsh.patch(Role::Tester, Path::new("/h"))).unwrap();
        assert!(!tester
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.get("insert").is_some()));
    }

    #[test]
    fn a_failed_turn_gives_its_error() {
        // What dsh 0.2.0-rc.2 printed when the API could not be reached.
        let stdout = r#"{"type":"session","sessionId":"session-1","cwd":"/w"}
{"type":"status","phase":"turn_start","turn":1}
{"type":"status","phase":"turn_end","turn":1,"reason":{"kind":"error","error":{"message":"DeepSeek Messages transport failed","code":"TRANSPORT"}}}
{"type":"final","text":""}"#;
        assert_eq!(
            turn_error(stdout).as_deref(),
            Some("dsh: TRANSPORT: DeepSeek Messages transport failed")
        );
        assert_eq!(turn_error("{\"type\":\"final\",\"text\":\"ok\"}"), None);
        assert_eq!(
            dsh_error("noise\ndsh: AUTH: invalid key\n").as_deref(),
            Some("dsh: AUTH: invalid key")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_run_reads_the_prompt_and_reports_success_by_exit_code() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        // A fake dsh: prints its home's key file name and what it read.
        let program = dir.path().join("dsh");
        fs::write(
            &program,
            "#!/bin/sh\ntask=$(cat)\n\
             test -f \"$DSH_HOME/.credentials.yaml\" || exit 2\n\
             echo '{\"type\":\"final\",\"text\":\"done: '\"$task\"'\"}'\n",
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let mut job = job(Role::Tester);
        job.project_dir = dir.path().to_path_buf();
        let dsh = Dsh::new(Secret::new("k"), RoleSettings::default()).with_program(&program);
        let outcome = dsh.run(&job).await;
        assert!(outcome.success, "{}", outcome.message);
        assert!(outcome.log.contains("done: do it"), "{}", outcome.log);
        assert!(outcome.log.starts_with("agent: dsh"), "{}", outcome.log);

        fs::write(
            &program,
            "#!/bin/sh\ncat >/dev/null\necho 'dsh: TRANSPORT: rate limit reached' >&2\nexit 1\n",
        )
        .unwrap();
        let outcome = dsh.run(&job).await;
        assert!(!outcome.success);
        assert!(outcome.usage_limit_reached);
        assert_eq!(outcome.message, "dsh: TRANSPORT: rate limit reached");
    }
}
