//! Runs a role with Google Antigravity CLI (`agy -p`), isolated from Lisa's own setup.
//!
//! Isolation (docs/design.md, section 5.4):
//! - an empty environment plus a short whitelist (see `process`), so no
//!   `GEMINI_API_KEY` reaches the agent;
//! - `agy` has no setting for its config folder, so it gets a fresh `HOME`: a
//!   temporary folder outside the project, deleted after the role. It holds a
//!   copy of the login saved by the Agents tab («Sign in») and our
//!   `settings.json` and `mcp_config.json`, and nothing else: no MCP servers,
//!   plugins or skills of Lisa's (only the role's own MCP servers from
//!   harness.toml), and no history of earlier runs;
//! - the login is not one file but a few of `agy`'s own files, so the whole
//!   saved folder is copied, without logs and old conversations. A run does
//!   not change the login files (checked with agy 1.2.12), so nothing is
//!   copied back;
//! - every role runs with `--dangerously-skip-permissions`: in headless mode
//!   any command that is not allowed ends the whole run, so the agent never
//!   gets to write its handoff. What a role may not do is said with `deny`
//!   rules in `settings.json`; they hold even with that flag, and a denied
//!   command only fails, the run goes on. `allow` rules are not a whitelist
//!   in headless mode, so the git check after the role is what enforces which
//!   folders a role may change.

mod home;
mod output;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use harness_core::task::agent::{AgentOutcome, AgentRunner, RoleJob};
use harness_core::task::handoff::Role;

use crate::process::{self, failed};
use crate::role_settings::RoleSettings;
pub(crate) use home::copy_dir;
use home::{settings, toolchain_env};
use output::{agy_error, last_line, RunResult};

/// Where `agy` keeps its settings, inside `HOME`.
const SETTINGS_DIR: &str = ".gemini/antigravity-cli";
/// Where `agy` reads the user's MCP servers, inside `HOME` (agy 1.2.12:
/// `agy mcp add` writes this file).
const MCP_CONFIG: &str = ".gemini/config/mcp_config.json";
/// Folders of the saved login that are history or logs, not the login itself.
const NOT_COPIED: &[&str] = &[
    "log",
    "brain",
    "conversations",
    "annotations",
    "presence",
    "crashes",
    "updater",
];

#[derive(Debug, Clone)]
pub struct Antigravity {
    program: PathBuf,
    /// `~/.harness/credentials/antigravity`: the `HOME` Lisa logged in with.
    auth_dir: PathBuf,
    /// Model, effort, MCP servers and time limit of each role (plugins are not used).
    settings: RoleSettings,
}

impl Antigravity {
    /// Antigravity with the login saved in `auth_dir`, running each role with its `settings`.
    pub fn new(auth_dir: impl Into<PathBuf>, settings: RoleSettings) -> Self {
        Self {
            program: PathBuf::from("agy"),
            auth_dir: auth_dir.into(),
            settings,
        }
    }

    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    /// Builds the full command for one job, without running it. `home` is the
    /// temporary `HOME` prepared by `prepare_home`.
    pub fn command(&self, job: &RoleJob, home: &Path) -> Command {
        let mut command = process::base_command(&self.program, &job.project_dir);
        // Some toolchains look for themselves under `$HOME`, which is replaced
        // below, so point them at the real folders first.
        if let Some(real_home) = harness_platform::home::home_dir() {
            for (name, path) in toolchain_env(&real_home) {
                command.env(name, path);
            }
        }
        harness_platform::home::set_for(&mut command, home);
        command
            .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
            .args(["-p", &job.prompt])
            .args(["--output-format", "stream-json"])
            .arg("--disable-slash-commands")
            .arg("--dangerously-skip-permissions");
        if let Some(model) = self.settings.model(job.role) {
            command.args(["--model", model]);
        }
        if let Some(effort) = self.settings.effort(job.role) {
            command.args(["--effort", effort]);
        }
        command
    }

    /// Makes a temporary `HOME` with a copy of the saved login and the role's
    /// `settings.json`. It is deleted when the returned value is dropped.
    fn prepare_home(&self, role: Role) -> io::Result<tempfile::TempDir> {
        if !self.auth_dir.join(SETTINGS_DIR).is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no Antigravity login saved; sign in on the Agents tab first",
            ));
        }
        let home = tempfile::Builder::new().prefix("harness-agy-").tempdir()?;
        copy_dir(&self.auth_dir, home.path())?;
        fs::write(
            home.path().join(SETTINGS_DIR).join("settings.json"),
            settings(role),
        )?;
        // Always written, so a server from the saved login never gets through.
        // The temporary HOME is readable only by Lisa, so secrets may be here.
        let mcp_config = home.path().join(MCP_CONFIG);
        if let Some(dir) = mcp_config.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(mcp_config, self.mcp_config(role))?;
        Ok(home)
    }

    /// `{"mcpServers": {"<name>": {"command", "args", "env"}}}`, agy's own format.
    fn mcp_config(&self, role: Role) -> String {
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
                    "command": server.command,
                    "args": server.args,
                    "env": env,
                    "disabled": false,
                });
                (server.name.clone(), spec)
            })
            .collect();
        serde_json::json!({ "mcpServers": servers }).to_string()
    }
}

impl AgentRunner for Antigravity {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        let log = self.settings.header("antigravity", job);
        let home = match self.prepare_home(job.role) {
            Ok(home) => home,
            Err(e) => return failed(log, format!("cannot prepare the agent's HOME: {e}")),
        };
        // The prompt is given with `-p`; standard input stays empty.
        let secrets = self.settings.server_secrets(job.role);
        let result = process::run(
            self.command(job, home.path()),
            "",
            self.settings.timeout(),
            &secrets,
        )
        .await;
        drop(home);
        let result = process::hide_secrets(result, secrets);
        outcome(log, result)
    }
}

/// Turns what `agy` printed into the outcome of the role.
fn outcome(mut log: String, result: Result<process::Finished, String>) -> AgentOutcome {
    let finished = match result {
        Ok(finished) => finished,
        Err(message) => return failed(log, message),
    };
    process::append_output(&mut log, &finished);

    let result = RunResult::find(&finished.stdout);
    let status = result.as_ref().map(|r| r.status.clone());
    let denied = result.map(|r| r.denied).unwrap_or_default();
    let success = finished.status.success() && status.as_deref() == Some("SUCCESS");
    let message = if success && denied.is_empty() {
        String::new()
    } else {
        match (&status, agy_error(&finished.stderr)) {
            (_, Some(error)) => error,
            // agy reports SUCCESS but stopped at the first action it could not
            // ask about, before the work was done.
            (Some(_), None) if !denied.is_empty() => format!(
                "agy stopped: it needed permission for {} and headless mode cannot ask",
                denied.join(", ")
            ),
            (Some(status), None) => format!("agy finished with status {status}"),
            (None, None) => format!(
                "agy exited ({}) without finishing: {}",
                finished.status,
                last_line(&finished.stderr)
            ),
        }
    };
    let usage_limit_reached =
        !success && process::looks_like_usage_limit(&format!("{message}\n{}", finished.stderr));
    let success = success && denied.is_empty();
    AgentOutcome {
        success: success && !usage_limit_reached,
        usage_limit_reached,
        log,
        message,
    }
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

    #[test]
    fn agy_runs_headless_with_its_own_home() {
        let agy = Antigravity::new("/creds/antigravity", RoleSettings::default());
        let command = agy.command(&job(Role::Developer), Path::new("/tmp/home"));
        let args = args(&command);
        assert_eq!(&args[..2], ["-p", "do it"]);
        assert!(args.contains(&"stream-json".to_string()), "{args:?}");
        let home = command
            .get_envs()
            .find(|(k, _)| *k == "HOME")
            .and_then(|(_, v)| v)
            .unwrap();
        assert_eq!(home, "/tmp/home");
    }

    #[test]
    fn every_role_runs_commands_and_is_limited_by_deny_rules() {
        let agy = Antigravity::new("/creds", RoleSettings::default());
        let home = Path::new("/tmp/home");
        let flag = "--dangerously-skip-permissions".to_string();
        for role in [Role::Architect, Role::Developer, Role::Security] {
            assert!(
                args(&agy.command(&job(role), home)).contains(&flag),
                "{role:?}"
            );
        }
        assert!(settings(Role::Security).contains("command(rm)"));
        assert!(settings(Role::Architect).contains("command(curl)"));
        assert!(!settings(Role::Developer).contains("command(rm)"));
        assert!(!settings(Role::Tester).contains("cargo build"));
    }

    #[test]
    fn no_api_keys_in_the_environment() {
        let command = Antigravity::new("/creds", RoleSettings::default())
            .command(&job(Role::Tester), Path::new("/h"));
        for (name, _) in command.get_envs() {
            let name = name.to_string_lossy();
            assert!(
                harness_platform::env::is_inherited(&name) || name == "AGY_CLI_DISABLE_AUTO_UPDATE",
                "{name} should not be passed"
            );
        }
    }

    #[test]
    fn every_role_is_denied_commits() {
        for role in [
            Role::Architect,
            Role::Developer,
            Role::Tester,
            Role::Security,
        ] {
            let parsed: serde_json::Value = serde_json::from_str(&settings(role)).unwrap();
            let deny = parsed["permissions"]["deny"].as_array().unwrap();
            assert!(deny.contains(&"command(git commit)".into()), "{role:?}");
            assert!(deny.contains(&"command(git push)".into()), "{role:?}");
        }
    }

    #[test]
    fn the_last_result_event_decides() {
        let ok = "{\"event\":\"init\"}\n\
                  {\"event\":\"result\",\"result\":{\"status\":\"SUCCESS\",\"response\":\"OK\"}}";
        assert_eq!(
            RunResult::find(ok),
            Some(RunResult {
                status: "SUCCESS".into(),
                denied: vec![]
            })
        );
        assert_eq!(RunResult::find("not json\n"), None);
        let stderr = "something\nAGY_ERROR: {\"status\":\"RESOURCE_EXHAUSTED\"}\n";
        assert_eq!(
            agy_error(stderr).as_deref(),
            Some("{\"status\":\"RESOURCE_EXHAUSTED\"}")
        );
    }

    #[test]
    fn toolchains_are_found_in_the_real_home() {
        let real_home = tempfile::tempdir().unwrap();
        fs::create_dir(real_home.path().join(".rustup")).unwrap();
        fs::create_dir(real_home.path().join("go")).unwrap();
        let env = toolchain_env(real_home.path());
        if std::env::var_os("RUSTUP_HOME").is_none() {
            assert!(env.contains(&("RUSTUP_HOME", real_home.path().join(".rustup"))));
        }
        if std::env::var_os("GOPATH").is_none() {
            assert!(env.contains(&("GOPATH", real_home.path().join("go"))));
        }
        // No `.cargo` folder there, so nothing is invented.
        assert!(!env.iter().any(|(name, _)| *name == "CARGO_HOME"));
    }

    #[test]
    fn the_login_is_copied_without_history() {
        let saved = tempfile::tempdir().unwrap();
        let settings_dir = saved.path().join(SETTINGS_DIR);
        fs::create_dir_all(settings_dir.join("implicit")).unwrap();
        fs::create_dir_all(settings_dir.join("conversations")).unwrap();
        fs::write(settings_dir.join("implicit/login.pb"), "secret").unwrap();
        fs::write(settings_dir.join("conversations/old.db"), "history").unwrap();
        fs::write(
            settings_dir.join("settings.json"),
            "{\"trustedWorkspaces\":[]}",
        )
        .unwrap();

        let home = Antigravity::new(saved.path(), RoleSettings::default())
            .prepare_home(Role::Architect)
            .unwrap();
        let copy = home.path().join(SETTINGS_DIR);
        assert!(copy.join("implicit/login.pb").exists());
        assert!(!copy.join("conversations").exists());
        let written = fs::read_to_string(copy.join("settings.json")).unwrap();
        assert!(written.contains("git commit"), "{written}");
        // The saved login is left as it was.
        assert!(fs::read_to_string(settings_dir.join("settings.json"))
            .unwrap()
            .contains("trustedWorkspaces"));

        let path = home.path().to_path_buf();
        drop(home);
        assert!(!path.exists());
    }

    #[test]
    fn the_home_has_only_the_roles_own_mcp_servers() {
        use crate::install::credentials::Secret;
        use std::collections::BTreeMap;

        let saved = tempfile::tempdir().unwrap();
        fs::create_dir_all(saved.path().join(SETTINGS_DIR)).unwrap();
        // A server Lisa added to her own login must not reach any role.
        fs::create_dir_all(saved.path().join(".gemini/config")).unwrap();
        fs::write(
            saved.path().join(MCP_CONFIG),
            r#"{"mcpServers":{"personal":{"command":"x"}}}"#,
        )
        .unwrap();
        let server = McpServer {
            name: "context7".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@upstash/context7-mcp".into()],
            env: BTreeMap::from([("CONTEXT7_API_KEY".into(), Secret::new("ctx-secret"))]),
        };
        let agent = Antigravity::new(
            saved.path(),
            RoleSettings::default().with_mcp_servers(Role::Security, vec![server]),
        );

        let read = |role| {
            let home = agent.prepare_home(role).unwrap();
            let text = fs::read_to_string(home.path().join(MCP_CONFIG)).unwrap();
            serde_json::from_str::<serde_json::Value>(&text).unwrap()
        };
        let security = read(Role::Security);
        assert_eq!(security["mcpServers"]["context7"]["command"], "npx");
        assert_eq!(
            security["mcpServers"]["context7"]["env"]["CONTEXT7_API_KEY"],
            "ctx-secret"
        );
        assert!(security["mcpServers"].get("personal").is_none());
        assert_eq!(read(Role::Tester), serde_json::json!({"mcpServers": {}}));
    }
}
