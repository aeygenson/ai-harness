//! Runs a role with Google Antigravity CLI (`agy -p`), isolated from Lisa's own setup.
//!
//! Isolation (docs/design.md, section 5.4):
//! - an empty environment plus a short whitelist (see `process`), so no
//!   `GEMINI_API_KEY` reaches the agent;
//! - `agy` has no setting for its config folder, so it gets a fresh `HOME`: a
//!   temporary folder outside the project, deleted after the role. It holds a
//!   copy of the login saved by `harness login antigravity` and our
//!   `settings.json`, and nothing else: no MCP servers, plugins or skills of
//!   Lisa's, and no history of earlier runs;
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

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use harness_core::agent::{AgentOutcome, AgentRunner, RoleJob};
use harness_core::handoff::Role;

use crate::process::{self, failed};

/// Where `agy` keeps its settings, inside `HOME`.
const SETTINGS_DIR: &str = ".gemini/antigravity-cli";
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
    models: HashMap<Role, String>,
    timeout: Duration,
}

impl Antigravity {
    pub fn new(auth_dir: impl Into<PathBuf>) -> Self {
        Self {
            program: PathBuf::from("agy"),
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

    /// Builds the full command for one job, without running it. `home` is the
    /// temporary `HOME` prepared by `prepare_home`.
    pub fn command(&self, job: &RoleJob, home: &Path) -> Command {
        let mut command = process::base_command(&self.program, &job.project_dir);
        command
            .env("HOME", home)
            .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
            .args(["-p", &job.prompt])
            .args(["--output-format", "stream-json"])
            .arg("--disable-slash-commands")
            .arg("--dangerously-skip-permissions");
        if let Some(model) = self.models.get(&job.role) {
            command.args(["--model", model]);
        }
        command
    }

    /// Makes a temporary `HOME` with a copy of the saved login and the role's
    /// `settings.json`. It is deleted when the returned value is dropped.
    fn prepare_home(&self, role: Role) -> io::Result<tempfile::TempDir> {
        if !self.auth_dir.join(SETTINGS_DIR).is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no Antigravity login saved; run `harness login antigravity` first",
            ));
        }
        let home = tempfile::Builder::new().prefix("harness-agy-").tempdir()?;
        copy_dir(&self.auth_dir, home.path())?;
        fs::write(
            home.path().join(SETTINGS_DIR).join("settings.json"),
            settings(role),
        )?;
        Ok(home)
    }

    fn header(&self, job: &RoleJob) -> String {
        let model = self.models.get(&job.role).map_or("default", String::as_str);
        format!(
            "agent: antigravity, model: {model}, role: {:?}, round: {}\n",
            job.role, job.round
        )
    }
}

impl AgentRunner for Antigravity {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        let log = self.header(job);
        let home = match self.prepare_home(job.role) {
            Ok(home) => home,
            Err(e) => return failed(log, format!("cannot prepare the agent's HOME: {e}")),
        };
        // The prompt is given with `-p`; standard input stays empty.
        let result = process::run(self.command(job, home.path()), "", self.timeout).await;
        drop(home);
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

/// Commands no role may run: the harness makes the commits.
const DENIED_FOR_ALL: &[&str] = &["git commit", "git push"];

/// Commands the roles that only look (Architect, Security) may not run: they
/// change files, the git state or reach the network. Reading and `cargo test`
/// stay allowed. The list cannot be complete; the git check after the role
/// still catches any changed file.
const DENIED_FOR_READERS: &[&str] = &[
    "rm",
    "mv",
    "cp",
    "git add",
    "git checkout",
    "git reset",
    "git restore",
    "git stash",
    "cargo build",
    "cargo install",
    "cargo run",
    "cargo publish",
    "curl",
    "wget",
];

/// The role's `settings.json`. `deny` beats everything else.
fn settings(role: Role) -> String {
    let mut deny: Vec<&str> = DENIED_FOR_ALL.to_vec();
    if matches!(role, Role::Architect | Role::Security) {
        deny.extend(DENIED_FOR_READERS);
    }
    let deny: Vec<String> = deny.iter().map(|c| format!("command({c})")).collect();
    let settings = serde_json::json!({
        "permissions": { "allow": [], "deny": deny },
        "allowNonWorkspaceAccess": false,
    });
    serde_json::to_string_pretty(&settings).expect("settings are plain JSON")
}

/// The last `{"event":"result","result":{"status":...,"denied_actions":[...]}}`.
#[derive(Debug, PartialEq, Eq)]
struct RunResult {
    status: String,
    /// Actions agy refused because headless mode cannot ask, e.g. "RunCommand".
    denied: Vec<String>,
}

impl RunResult {
    fn find(stdout: &str) -> Option<Self> {
        stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| event["event"] == "result")
            .filter_map(|event| {
                let result = &event["result"];
                let status = result["status"].as_str()?.to_string();
                let denied = result["denied_actions"]
                    .as_array()
                    .map(|actions| {
                        actions
                            .iter()
                            .map(|a| {
                                a["display_name"]
                                    .as_str()
                                    .or(a["action"].as_str())
                                    .unwrap_or("an action")
                                    .to_string()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some(RunResult { status, denied })
            })
            .next_back()
    }
}

/// A failed call to the model is reported on stderr as `AGY_ERROR: {...}`.
fn agy_error(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("AGY_ERROR:"))
        .map(|rest| rest.trim().to_string())
}

fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
}

/// Copies the saved login folder, leaving out logs and history.
fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if NOT_COPIED.contains(&name.to_string_lossy().as_ref()) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &to.join(&name))?;
        } else if kind.is_file() {
            fs::copy(entry.path(), to.join(&name))?;
        }
    }
    Ok(())
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
    fn agy_runs_headless_with_its_own_home() {
        let agy = Antigravity::new("/creds/antigravity");
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
        let agy = Antigravity::new("/creds");
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
        let command = Antigravity::new("/creds").command(&job(Role::Tester), Path::new("/h"));
        for (name, _) in command.get_envs() {
            let name = name.to_string_lossy();
            assert!(
                process::INHERITED_ENV.contains(&name.as_ref())
                    || name == "AGY_CLI_DISABLE_AUTO_UPDATE",
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

        let home = Antigravity::new(saved.path())
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
}
