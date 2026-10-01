//! Builds the agents that `harness.toml` asks for: each role's agent with its
//! model, MCP servers and plugins, and the `[retro]` agent. The command line
//! and the TUI both start roles with these functions.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use harness_core::config::Config;
use harness_core::handoff::Role;
use harness_core::mcp::{McpServer, McpServers};
use harness_core::plugins::{Plugin, Plugins};
use harness_core::suggest;

use crate::credentials::{self, Secret};
use crate::{codex, Antigravity, AnyAgent, ClaudeCode, Codex, Team};

/// Why an agent cannot be built, in words for Lisa: a missing login, an
/// unknown agent name, a broken setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildError(pub String);

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BuildError {}

fn problem(text: impl Into<String>) -> BuildError {
    BuildError(text.into())
}

/// The four roles that run agents, in the order of the flow.
pub const ROLES: [Role; 4] = [
    Role::Architect,
    Role::Developer,
    Role::Tester,
    Role::Security,
];

/// The team from harness.toml: each role gets the agent, model, MCP servers
/// and plugins set there.
pub fn build_team(config: &Config, project_dir: &Path) -> Result<Team, BuildError> {
    let dir = credentials::default_dir().ok_or_else(|| problem("HOME is not set"))?;
    let servers = McpServers::load(config, |name| credentials::load_secret(&dir, name).ok())
        .map_err(|e| problem(e.to_string()))?;
    let plugins = Plugins::load(project_dir, config).map_err(|e| problem(e.to_string()))?;
    let mut team = Team::new();
    for role in ROLES {
        let settings = config.role(role).map_err(|e| problem(e.to_string()))?;
        let agent = build_agent(
            config,
            &AgentChoice {
                who: &format!("{role:?}"),
                agent: &settings.agent,
                model: settings.model.as_deref(),
                effort: settings.effort.as_deref(),
                role,
            },
            servers.for_role(role),
            plugins.for_role(role),
        )?;
        team = team.with(role, agent);
    }
    Ok(team)
}

/// The `[retro]` agent: the read-only rules of the security role, and no MCP
/// servers or plugins.
pub fn retro_agent(config: &Config) -> Result<AnyAgent, BuildError> {
    let Some(settings) = &config.retro else {
        return Err(problem(
            "harness.toml has no [retro] section; add\n[retro]\nagent = \"claude\"",
        ));
    };
    build_agent(
        config,
        &AgentChoice {
            who: "[retro]",
            agent: &settings.agent,
            model: settings.model.as_deref(),
            effort: settings.effort.as_deref(),
            role: suggest::RULES_OF,
        },
        Vec::new(),
        Vec::new(),
    )
}

/// Which agent to build, and for whom.
pub struct AgentChoice<'a> {
    /// For error messages: `Tester` or `[retro]`.
    pub who: &'a str,
    pub agent: &'a str,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    /// The role whose rules the agent gets.
    pub role: Role,
}

pub fn build_agent(
    config: &Config,
    choice: &AgentChoice,
    servers: Vec<McpServer>,
    plugins: Vec<Plugin>,
) -> Result<AnyAgent, BuildError> {
    let dir = credentials::default_dir().ok_or_else(|| problem("HOME is not set"))?;
    // Codex starts MCP servers and reads keys through this same program.
    let harness = std::env::current_exe()
        .map_err(|e| problem(format!("cannot find the harness program: {e}")))?;
    let timeout = Duration::from_secs(config.agent_timeout_minutes * 60);
    let role = choice.role;
    let agent = match choice.agent {
        "claude" => {
            let token = credentials::load_token(&dir, "claude").map_err(|_| {
                problem("no Claude token saved; run `harness login claude` first")
            })?;
            let mut agent = ClaudeCode::new(token).with_timeout(timeout);
            if let Some(model) = choice.model {
                agent = agent.with_model(role, model);
            }
            if let Some(effort) = choice.effort {
                agent = agent.with_effort(role, effort);
            }
            AnyAgent::Claude(
                agent
                    .with_mcp_servers(role, servers)
                    .with_plugins(role, plugins),
            )
        }
        "codex" => {
            let auth_dir = dir.join("codex");
            if !auth_dir.join("auth.json").exists() {
                return Err(problem(
                    "no Codex login saved; run `harness login codex` first",
                ));
            }
            let mut agent = Codex::new(auth_dir).with_timeout(timeout);
            if let Some(model) = choice.model {
                agent = agent.with_model(role, model);
            }
            if let Some(effort) = choice.effort {
                agent = agent.with_effort(role, effort);
            }
            AnyAgent::Codex(
                agent
                    .with_launcher(&harness)
                    .with_mcp_servers(role, servers)
                    .with_plugins(role, plugins),
            )
        }
        "codex+deepseek" => {
            // A key in the shell wins; otherwise the one `harness login deepseek` saved.
            let key = match std::env::var(codex::DEEPSEEK_KEY_ENV) {
                Ok(key) if !key.trim().is_empty() => Secret::new(key.trim()),
                _ => credentials::load_token(&dir, "deepseek").map_err(|_| {
                    problem("no DeepSeek API key saved; run `harness login deepseek` first")
                })?,
            };
            let mut agent = Codex::deepseek(key).with_timeout(timeout);
            if let Some(model) = choice.model {
                agent = agent.with_model(role, model);
            }
            if choice.effort.is_some() {
                return Err(problem(format!(
                    "{} uses codex+deepseek, which has no effort levels: remove `effort`",
                    choice.who
                )));
            }
            AnyAgent::Codex(
                agent
                    .with_launcher(&harness)
                    .with_mcp_servers(role, servers)
                    .with_plugins(role, plugins),
            )
        }
        "antigravity" => {
            let auth_dir = dir.join("antigravity");
            if !auth_dir.join(".gemini/antigravity-cli").is_dir() {
                return Err(problem(
                    "no Antigravity login saved; run `harness login antigravity` first",
                ));
            }
            let mut agent = Antigravity::new(auth_dir).with_timeout(timeout);
            if let Some(model) = choice.model {
                agent = agent.with_model(role, model);
            }
            if let Some(effort) = choice.effort {
                agent = agent.with_effort(role, effort);
            }
            AnyAgent::Antigravity(agent.with_mcp_servers(role, servers))
        }
        other => {
            return Err(problem(format!(
                "{} uses agent {other:?}; use \"claude\", \"codex\", \"codex+deepseek\" or \"antigravity\"",
                choice.who
            )))
        }
    };
    Ok(agent)
}
