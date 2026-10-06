//! A team: each role runs with its own agent, as chosen in `harness.toml`.
//! For example the Developer on Claude Code and the Tester on Codex.

use std::collections::HashMap;

use harness_core::task::agent::{AgentOutcome, AgentRunner, RoleJob};
use harness_core::task::handoff::Role;

use crate::process::failed;
use crate::{Antigravity, ClaudeCode, Codex, Dsh, MockAgent};

/// One of the agents the harness knows.
///
/// Why an enum and not `Box<dyn AgentRunner>`? Our trait returns
/// `impl Future`, and such traits cannot be used as `dyn`. An enum with a
/// `match` is simple and the compiler checks that every agent is handled.
#[derive(Debug)]
pub enum AnyAgent {
    /// Claude Code.
    Claude(ClaudeCode),
    /// Codex CLI.
    Codex(Codex),
    /// Antigravity CLI.
    Antigravity(Antigravity),
    /// `DeepSeek` Harness.
    Dsh(Dsh),
    /// The scripted mock agent, for tests.
    Mock(MockAgent),
}

impl AgentRunner for AnyAgent {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        match self {
            AnyAgent::Claude(agent) => agent.run(job).await,
            AnyAgent::Codex(agent) => agent.run(job).await,
            AnyAgent::Antigravity(agent) => agent.run(job).await,
            AnyAgent::Dsh(agent) => agent.run(job).await,
            AnyAgent::Mock(agent) => agent.run(job).await,
        }
    }
}

/// The agent of each role; running a role with no agent set fails with a message.
#[derive(Debug, Default)]
pub struct Team {
    agents: HashMap<Role, AnyAgent>,
}

impl Team {
    /// A team with no agents yet; add them with `with`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lets `agent` run `role`, replacing the agent set for it before.
    pub fn with(mut self, role: Role, agent: AnyAgent) -> Self {
        self.agents.insert(role, agent);
        self
    }
}

impl AgentRunner for Team {
    async fn run(&self, job: &RoleJob) -> AgentOutcome {
        match self.agents.get(&job.role) {
            Some(agent) => agent.run(job).await,
            None => failed(
                String::new(),
                format!("no agent is set for the {:?} role", job.role),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockStep;
    use harness_core::task::agent::RunEnd;
    use harness_core::task::handoff::{NextStep, Verdict};
    use std::path::PathBuf;

    fn job(role: Role, dir: &std::path::Path) -> RoleJob {
        RoleJob {
            task_id: "task-001".into(),
            round: 1,
            role,
            project_dir: dir.to_path_buf(),
            prompt: String::new(),
            output_dir: PathBuf::from(dir),
        }
    }

    #[tokio::test]
    async fn each_role_goes_to_its_own_agent() {
        let dir = tempfile::tempdir().unwrap();
        let tester = MockAgent::new().then(
            Role::Tester,
            MockStep::finish(Verdict::Approved, NextStep::To(Role::Security)),
        );
        let team = Team::new().with(Role::Tester, AnyAgent::Mock(tester));

        let outcome = team.run(&job(Role::Tester, dir.path())).await;
        assert_eq!(outcome.end, RunEnd::Succeeded);

        let outcome = team.run(&job(Role::Developer, dir.path())).await;
        assert!(
            matches!(&outcome.end, RunEnd::Failed(message) if message.contains("Developer")),
            "{:?}",
            outcome.end
        );
    }
}
