//! Runs a role with Google's Gemini CLI, which speaks ACP (`gemini --acp`).
//!
//! Isolation, as for Antigravity (docs/design.md, section 5.4):
//! - an empty environment plus a short whitelist (see `process`), so no
//!   `GEMINI_API_KEY` from the user's shell reaches the agent;
//! - a fresh `HOME`: a temporary folder outside the project, deleted after
//!   the role. It holds only the Google login saved by the Agents tab («Sign
//!   in», `~/.harness/credentials/gemini`) and our own `settings.json`, so no
//!   MCP servers, extensions, memory or history of the user's own Gemini setup
//!   are used; the role's MCP servers come with the session (see `acp`);
//! - Gemini asks before it changes a file or runs a command, and the
//!   harness answers by the role's rights (see `acp::permission`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use harness_core::config::AgentKind;
use harness_core::task::agent::{AgentOutcome, AgentRunner, RoleJob};

use crate::adapters::acp::{run_role, Speaker};
use crate::adapters::antigravity::toolchain_env;
use crate::process::{self, failed};
use crate::role_settings::RoleSettings;

/// Gemini's folder inside `HOME`.
pub const SETTINGS_DIR: &str = ".gemini";
/// The Google login inside [`SETTINGS_DIR`]: written by «Sign in with Google».
pub const LOGIN_FILE: &str = "oauth_creds.json";
/// The other files of the saved login that are copied to a role's `HOME`.
/// Everything else there (history, chats, logs) is left out.
const LOGIN_FILES: &[&str] = &[LOGIN_FILE, "google_accounts.json", "installation_id"];
/// How the user signed in, as Gemini writes it into its `settings.json`.
const GOOGLE_LOGIN: &str = "oauth-personal";
/// What to do when Gemini says it is not signed in.
const SIGN_IN: &str = "sign in on the Agents tab («Sign in», with Google) and run the role again";

/// Runs a role with Gemini CLI, logged in through its own `HOME`.
#[derive(Debug, Clone)]
pub struct Gemini {
    program: PathBuf,
    /// `~/.harness/credentials/gemini`: the `HOME` the user signed in with.
    auth_dir: PathBuf,
    /// Model, MCP servers and time limit of each role (no effort, no plugins).
    settings: RoleSettings,
}

impl Gemini {
    /// Gemini with the login saved in `auth_dir`, running each role with its `settings`.
    pub fn new(auth_dir: impl Into<PathBuf>, settings: RoleSettings) -> Self {
        Self {
            program: PathBuf::from("gemini"),
            auth_dir: auth_dir.into(),
            settings,
        }
    }

    /// Uses `program` instead of `gemini`; tests pass a fake program.
    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    /// Builds the full command for one job, without running it. `home` is
    /// the temporary `HOME` prepared by [`prepare_home`].
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
        // `default`: Gemini asks before changing files or running commands.
        command.args(["--acp", "--approval-mode", "default"]);
        if let Some(model) = self.settings.model(job.role) {
            command.args(["--model", model]);
        }
        command
    }
}

impl AgentRunner for Gemini {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        let home = match prepare_home(&self.auth_dir) {
            Ok(home) => home,
            Err(e) => {
                let log = self.settings.header(AgentKind::Gemini, job);
                return failed(log, format!("cannot prepare the agent's HOME: {e}"));
            }
        };
        let speaker = Speaker {
            kind: AgentKind::Gemini,
            sign_in: SIGN_IN,
        };
        let command = self.command(job, home.path());
        let outcome = run_role(speaker, command, job, &self.settings).await;
        drop(home);
        outcome
    }
}

/// Makes a temporary `HOME` with a copy of the saved login and our own
/// `settings.json`. It is deleted when the returned value is dropped.
pub fn prepare_home(auth_dir: &Path) -> io::Result<tempfile::TempDir> {
    let saved = auth_dir.join(SETTINGS_DIR);
    if !saved.join(LOGIN_FILE).is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no Gemini login saved; sign in on the Agents tab first",
        ));
    }
    let home = tempfile::Builder::new()
        .prefix("harness-gemini-")
        .tempdir()?;
    let settings_dir = home.path().join(SETTINGS_DIR);
    fs::create_dir_all(&settings_dir)?;
    for name in LOGIN_FILES {
        let from = saved.join(name);
        if from.is_file() {
            fs::copy(from, settings_dir.join(name))?;
        }
    }
    fs::write(settings_dir.join("settings.json"), settings())?;
    Ok(home)
}

/// Our `settings.json`: the Google login, no update checks while a role works.
fn settings() -> String {
    let settings = serde_json::json!({
        "security": { "auth": { "selectedType": GOOGLE_LOGIN } },
        "general": { "disableAutoUpdate": true, "disableUpdateNag": true },
    });
    serde_json::to_string_pretty(&settings).expect("settings are plain JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved_login(dir: &Path) {
        let gemini = dir.join(SETTINGS_DIR);
        fs::create_dir_all(gemini.join("tmp/chats")).unwrap();
        fs::write(gemini.join(LOGIN_FILE), "{\"refresh_token\":\"r\"}").unwrap();
        fs::write(
            gemini.join("settings.json"),
            "{\"mcpServers\":{\"mine\":{}}}",
        )
        .unwrap();
        fs::write(gemini.join("tmp/chats/old.json"), "history").unwrap();
    }

    #[test]
    fn a_role_s_home_has_the_login_and_our_settings_only() {
        let auth = tempfile::tempdir().unwrap();
        saved_login(auth.path());

        let home = prepare_home(auth.path()).unwrap();

        let gemini = home.path().join(SETTINGS_DIR);
        assert!(gemini.join(LOGIN_FILE).is_file());
        assert!(!gemini.join("tmp").exists());
        let settings = fs::read_to_string(gemini.join("settings.json")).unwrap();
        assert!(settings.contains(GOOGLE_LOGIN), "{settings}");
        assert!(!settings.contains("mine"), "{settings}");
    }

    #[test]
    fn without_a_saved_login_no_home_is_made() {
        let auth = tempfile::tempdir().unwrap();

        let error = prepare_home(auth.path()).unwrap_err();

        assert!(error.to_string().contains("sign in"), "{error}");
    }

    #[test]
    fn the_command_speaks_acp_and_asks_before_acting() {
        let settings = RoleSettings::default().with_model(
            harness_core::task::handoff::Role::Developer,
            "gemini-2.5-pro",
        );
        let gemini = Gemini::new("/auth", settings);
        let job = RoleJob {
            task_id: "task-001".into(),
            round: 1,
            role: harness_core::task::handoff::Role::Developer,
            project_dir: PathBuf::from("/p"),
            prompt: "do it".into(),
            output_dir: PathBuf::from("/p/out"),
            stop: harness_core::task::stop_request::StopRequest::new(),
        };

        let command = gemini.command(&job, Path::new("/home-tmp"));

        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--acp",
                "--approval-mode",
                "default",
                "--model",
                "gemini-2.5-pro"
            ]
        );
        // The prompt goes over the protocol, never on the command line.
        assert!(!args.iter().any(|a| a.contains("do it")));
    }
}
