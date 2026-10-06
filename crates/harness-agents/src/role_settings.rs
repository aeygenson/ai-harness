//! What each role gets from `harness.toml`, the same for every agent: model,
//! reasoning effort, MCP servers, plugins and the time limit.
//!
//! Every adapter keeps one [`RoleSettings`] and reads it while building its
//! command, so these settings are stored and looked up in one place.

use std::collections::HashMap;
use std::time::Duration;

use harness_core::config::AgentKind;
use harness_core::mcp::McpServer;
use harness_core::plugins::Plugin;
use harness_core::task::agent::RoleJob;
use harness_core::task::handoff::Role;

/// How long a role may run when `harness.toml` does not say: 30 minutes,
/// enough for a large change, short enough to notice a stuck agent.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The per-role settings an adapter runs with. Built with the `with_*`
/// methods, like the adapters themselves.
#[derive(Debug, Clone)]
pub struct RoleSettings {
    models: HashMap<Role, String>,
    efforts: HashMap<Role, String>,
    mcp: HashMap<Role, Vec<McpServer>>,
    plugins: HashMap<Role, Vec<Plugin>>,
    timeout: Duration,
}

impl Default for RoleSettings {
    fn default() -> Self {
        Self {
            models: HashMap::new(),
            efforts: HashMap::new(),
            mcp: HashMap::new(),
            plugins: HashMap::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl RoleSettings {
    /// The model a role uses, such as "gpt-5.5"; without one the agent's default.
    pub fn with_model(mut self, role: Role, model: impl Into<String>) -> Self {
        self.models.insert(role, model.into());
        self
    }

    /// The reasoning effort of a role, such as "high".
    pub fn with_effort(mut self, role: Role, effort: impl Into<String>) -> Self {
        self.efforts.insert(role, effort.into());
        self
    }

    /// How long one role may run before it is stopped.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The MCP servers this role gets (from harness.toml).
    pub fn with_mcp_servers(mut self, role: Role, servers: Vec<McpServer>) -> Self {
        self.mcp.insert(role, servers);
        self
    }

    /// The plugins this role gets (from harness.toml). Plugins belong to one
    /// agent, so only the adapters that support them (Claude, Codex) read them.
    pub fn with_plugins(mut self, role: Role, plugins: Vec<Plugin>) -> Self {
        self.plugins.insert(role, plugins);
        self
    }

    /// The model of `role`, or `None` for the agent's default.
    pub fn model(&self, role: Role) -> Option<&str> {
        self.models.get(&role).map(String::as_str)
    }

    /// The reasoning effort of `role`, or `None` for the agent's default.
    pub fn effort(&self, role: Role) -> Option<&str> {
        self.efforts.get(&role).map(String::as_str)
    }

    /// The MCP servers of `role` (empty when it has none).
    pub fn servers(&self, role: Role) -> &[McpServer] {
        self.mcp.get(&role).map_or(&[], Vec::as_slice)
    }

    /// The plugins of `role` (empty when it has none).
    pub fn plugins(&self, role: Role) -> &[Plugin] {
        self.plugins.get(&role).map_or(&[], Vec::as_slice)
    }

    /// How long one role may run.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The secret values in the settings of `role`'s MCP servers, so they can
    /// be replaced with `***` in everything the agent prints.
    pub fn server_secrets(&self, role: Role) -> Vec<&str> {
        self.servers(role)
            .iter()
            .flat_map(|server| server.env.values().map(|value| value.expose()))
            .collect()
    }

    /// The first line of a role's `agent.log`: which agent, model and effort
    /// ran which role in which round.
    pub fn header(&self, agent: AgentKind, job: &RoleJob) -> String {
        let model = self.model(job.role).unwrap_or("default");
        let effort = self.effort(job.role).unwrap_or("default");
        format!(
            "agent: {agent}, model: {model}, effort: {effort}, role: {:?}, round: {}\n",
            job.role, job.round
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use harness_core::secret::Secret;

    fn job(role: Role) -> RoleJob {
        RoleJob {
            task_id: "task-001".into(),
            round: 2,
            role,
            project_dir: PathBuf::from("/p"),
            prompt: String::new(),
            output_dir: PathBuf::from("/p/out"),
        }
    }

    #[test]
    fn each_role_keeps_its_own_settings() {
        let server = McpServer {
            name: "docs".into(),
            command: "npx".into(),
            args: Vec::new(),
            env: [("KEY".to_string(), Secret::new("key-12345678"))].into(),
        };
        let settings = RoleSettings::default()
            .with_model(Role::Developer, "big")
            .with_effort(Role::Developer, "high")
            .with_mcp_servers(Role::Tester, vec![server]);

        assert_eq!(settings.model(Role::Developer), Some("big"));
        assert_eq!(settings.model(Role::Tester), None);
        assert_eq!(settings.servers(Role::Developer).len(), 0);
        assert_eq!(settings.server_secrets(Role::Tester), ["key-12345678"]);
        assert!(settings.plugins(Role::Tester).is_empty());
        assert_eq!(settings.timeout(), DEFAULT_TIMEOUT);
        assert_eq!(
            settings.header(AgentKind::Codex, &job(Role::Developer)),
            "agent: codex, model: big, effort: high, role: Developer, round: 2\n"
        );
        assert_eq!(
            settings.header(AgentKind::Dsh, &job(Role::Tester)),
            "agent: dsh, model: default, effort: default, role: Tester, round: 2\n"
        );
    }
}
