//! Runs a role with Cursor CLI (`agent -p`), with its settings in a temporary folder.
//!
//! How a role is kept to its rights (compare docs/design.md, section 5.4):
//! - an empty environment plus a short whitelist (see `process`), so no
//!   `CURSOR_API_KEY` from the user's shell reaches the agent;
//! - the login is Cursor's own (`agent login`, saved by Cursor itself, on a
//!   Mac in the Keychain): the harness never sees or passes on a key;
//! - `CURSOR_CONFIG_DIR` points at a private temporary folder outside the
//!   project, deleted after the role. It holds only our `cli-config.json`
//!   (the role's deny rules, no "Made with Cursor" lines) and `mcp.json`
//!   (the role's own MCP servers), so nothing of the user's own Cursor
//!   setup is used;
//! - every role runs with `--force`: headless mode cannot ask, and without
//!   it changes are only proposed, never made. What a role may not do is
//!   said with `deny` rules, which hold even with `--force`; the git check
//!   after the role is what enforces which files a role may change.

mod config;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use harness_core::config::AgentKind;
use harness_core::task::agent::{AgentOutcome, AgentRunner, RoleJob};
use harness_core::task::handoff::Role;

use crate::adapters::antigravity::last_line;
use crate::adapters::claude::FinalResult;
use crate::process::{self, failed};
use crate::role_settings::RoleSettings;
use config::{cli_config, mcp_config};

/// The names of Cursor's program: the installer puts both in `~/.local/bin`
/// (older installs have only `cursor-agent`). `cursor-agent` is looked for
/// first, because some other tool may also be called `agent`.
pub const PROGRAMS: [&str; 2] = ["cursor-agent", "agent"];

/// The Cursor program found on this computer, or plain `agent` when neither
/// name is found (starting it then fails with a clear message).
pub fn program() -> PathBuf {
    PROGRAMS
        .iter()
        .find_map(|name| harness_platform::program::find(name))
        .unwrap_or_else(|| PathBuf::from(PROGRAMS[0]))
}

/// Runs a role with Cursor CLI, logged in with Cursor's own login.
#[derive(Debug, Clone)]
pub struct Cursor {
    program: PathBuf,
    /// Model, MCP servers and time limit of each role (no effort, no plugins).
    settings: RoleSettings,
}

impl Cursor {
    /// Cursor CLI running each role with its `settings`.
    pub fn new(settings: RoleSettings) -> Self {
        Self {
            program: program(),
            settings,
        }
    }

    /// Uses `program` instead of `agent`; tests pass a fake program.
    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    /// Builds the full command for one job, without running it. `config_dir`
    /// is the temporary folder prepared by `prepare_config`.
    pub fn command(&self, job: &RoleJob, config_dir: &Path) -> Command {
        let mut command = process::base_command(&self.program, &job.project_dir);
        command
            .env("CURSOR_CONFIG_DIR", config_dir)
            // A role never opens a browser; a missing login fails instead.
            .env("NO_OPEN_BROWSER", "1")
            .args(["-p", "--output-format", "stream-json"])
            .args(["--trust", "--force"])
            .arg("--workspace")
            .arg(&job.project_dir);
        if let Some(model) = self.settings.model(job.role) {
            command.args(["--model", model]);
        }
        // Cursor has no effort setting: it is part of the model's name
        // (for example `...-thinking`), so a role's `effort` is not used.
        if !self.settings.servers(job.role).is_empty() {
            command.arg("--approve-mcps");
        }
        command.arg(&job.prompt);
        command
    }

    /// Makes the temporary config folder with the role's `cli-config.json`
    /// and `mcp.json`. It is deleted when the returned value is dropped.
    fn prepare_config(&self, role: Role) -> io::Result<tempfile::TempDir> {
        let dir = tempfile::Builder::new()
            .prefix("harness-cursor-")
            .tempdir()?;
        let servers = self.settings.servers(role);
        fs::write(
            dir.path().join("cli-config.json"),
            cli_config(role, servers),
        )?;
        // Always written, so a server of the user's own never gets through.
        // `tempdir` makes a folder only the user can open, so secrets may be here.
        fs::write(dir.path().join("mcp.json"), mcp_config(servers))?;
        Ok(dir)
    }
}

impl AgentRunner for Cursor {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        let log = self.settings.header(AgentKind::Cursor, job);
        let config_dir = match self.prepare_config(job.role) {
            Ok(dir) => dir,
            Err(e) => return failed(log, format!("cannot prepare Cursor's settings: {e}")),
        };
        // The prompt is the last argument; standard input stays empty.
        let secrets = self.settings.server_secrets(job.role);
        let result = process::run(
            self.command(job, config_dir.path()),
            "",
            self.settings.timeout(),
            &secrets,
            &job.stop,
        )
        .await;
        drop(config_dir);
        let result = process::hide_secrets(result, secrets);
        outcome(log, result)
    }
}

/// Turns what `agent` printed into the outcome of the role. Its last event is
/// `{"type":"result","is_error":false,...}`; a failed run may end without one
/// and say why on standard error.
fn outcome(mut log: String, result: Result<process::Finished, String>) -> AgentOutcome {
    let finished = match result {
        Ok(finished) => finished,
        Err(message) => return failed(log, message),
    };
    process::append_output(&mut log, &finished);
    let result = FinalResult::find(&finished.stdout);
    let success = finished.status.success() && result.as_ref().is_some_and(|r| !r.is_error);
    let message = match &result {
        _ if success => String::new(),
        Some(result) if !result.text.is_empty() => result.text.clone(),
        _ => format!(
            "Cursor exited ({}) without a result: {}",
            finished.status,
            last_line(&finished.stderr)
        ),
    };
    let end = process::run_end(success, message, &finished.stderr);
    AgentOutcome { end, log }
}

#[cfg(test)]
mod tests;
