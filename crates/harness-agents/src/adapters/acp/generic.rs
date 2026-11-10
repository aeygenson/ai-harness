//! Any agent that speaks ACP, started with the command the user gives in
//! `harness.toml`:
//!
//! ```toml
//! [acp]
//! command = "opencode"
//! args = ["acp"]
//! ```
//!
//! The harness knows nothing else about such an agent, so it keeps the
//! agent's own login and settings in the user's home folder (an empty
//! environment plus the whitelist, as for every agent), and cannot choose
//! its model: the agent's own default is used.

use std::path::PathBuf;

use harness_core::config::{AcpConfig, AgentKind};
use harness_core::task::agent::{AgentOutcome, AgentRunner, RoleJob};

use super::{run_role, Speaker};
use crate::process;
use crate::role_settings::RoleSettings;

/// What to do when the agent says it is not signed in.
const SIGN_IN: &str = "sign in with the agent's own command in a terminal, then run the role again";

/// Runs a role with the ACP agent named in `[acp]`.
#[derive(Debug, Clone)]
pub struct Acp {
    program: PathBuf,
    args: Vec<String>,
    /// MCP servers and time limit of each role (no model, effort or plugins).
    settings: RoleSettings,
}

impl Acp {
    /// The agent `config` names, running each role with its `settings`.
    pub fn new(config: &AcpConfig, settings: RoleSettings) -> Self {
        Self {
            program: PathBuf::from(&config.command),
            args: config.args.clone(),
            settings,
        }
    }

    /// Uses `program` instead of the one in `[acp]`; tests pass a fake program.
    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }
}

impl AgentRunner for Acp {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        let mut command = process::base_command(&self.program, &job.project_dir);
        command.args(&self.args);
        let speaker = Speaker {
            kind: AgentKind::Acp,
            sign_in: SIGN_IN,
        };
        run_role(speaker, command, job, &self.settings).await
    }
}
