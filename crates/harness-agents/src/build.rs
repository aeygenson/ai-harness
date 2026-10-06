//! Builds the agents that `harness.toml` asks for: each role's agent with its
//! model, MCP servers and plugins, and the `[retro]` agent. The command line
//! and the TUI both start roles with these functions.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use harness_core::config::{AgentKind, Config};
use harness_core::mcp::{McpServer, McpServers};
use harness_core::plugins::{Plugin, Plugins};
use harness_core::retro::suggest;
use harness_core::task::handoff::Role;

use crate::install::credentials::{self, Secret};
use crate::role_settings::RoleSettings;
use crate::{adapters::dsh, Antigravity, AnyAgent, ClaudeCode, Codex, Dsh, Team};

/// Why an agent cannot be built, in words for Lisa: a missing login or a
/// broken setting.
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
    let servers = McpServers::load(config, |name| crate::mcp::oauth::mcp_secret(&dir, name))
        .map_err(|e| problem(e.to_string()))?;
    let plugins = Plugins::load(project_dir, config).map_err(|e| problem(e.to_string()))?;
    let mut team = Team::new();
    for role in ROLES {
        let settings = config.role(role).map_err(|e| problem(e.to_string()))?;
        let agent = build_agent(
            config,
            &AgentChoice {
                who: &format!("{role:?}"),
                agent: settings.agent,
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
            agent: settings.agent,
            model: settings.model.as_deref(),
            effort: settings.effort.as_deref(),
            role: suggest::RULES_OF,
        },
        Vec::new(),
        Vec::new(),
    )
}

/// Which agent to build, and for whom.
#[derive(Debug)]
pub struct AgentChoice<'a> {
    /// For error messages: `Tester` or `[retro]`.
    pub who: &'a str,
    /// Which agent program runs the role.
    pub agent: AgentKind,
    /// The model to ask for; `None` keeps the agent's own default.
    pub model: Option<&'a str>,
    /// The reasoning effort to ask for; `None` keeps the agent's own default.
    pub effort: Option<&'a str>,
    /// The role whose rules the agent gets.
    pub role: Role,
}

/// Builds the agent `choice` names, with the role's model, effort, MCP
/// servers and plugins. Fails with a message for Lisa when the agent's
/// login is missing.
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
    let settings = role_settings(config, choice, &harness, servers, plugins);
    match choice.agent {
        AgentKind::Claude => Ok(AnyAgent::Claude(ClaudeCode::new(
            claude_token(&dir)?,
            settings,
        ))),
        AgentKind::Codex => {
            let codex = Codex::new(codex_login(&dir)?, settings).with_launcher(&harness);
            Ok(AnyAgent::Codex(codex))
        }
        AgentKind::Antigravity => Ok(AnyAgent::Antigravity(Antigravity::new(
            antigravity_login(&dir)?,
            settings,
        ))),
        AgentKind::Dsh => {
            check_dsh_effort(choice)?;
            Ok(AnyAgent::Dsh(Dsh::new(deepseek_key(&dir)?, settings)))
        }
    }
}

/// The settings every agent gets: the role's model and effort from `choice`,
/// its MCP servers (through the harness's bridge) and plugins, and the time
/// limit from `harness.toml`.
fn role_settings(
    config: &Config,
    choice: &AgentChoice,
    harness: &Path,
    servers: Vec<McpServer>,
    plugins: Vec<Plugin>,
) -> RoleSettings {
    let role = choice.role;
    let mut settings = RoleSettings::default()
        .with_timeout(Duration::from_secs(config.agent_timeout_minutes * 60))
        .with_mcp_servers(role, crate::launcher::with_bridge(servers, harness))
        .with_plugins(role, plugins);
    if let Some(model) = choice.model {
        settings = settings.with_model(role, model);
    }
    if let Some(effort) = choice.effort {
        settings = settings.with_effort(role, effort);
    }
    settings
}

/// The Claude token the Agents tab («Sign in») saved.
fn claude_token(dir: &Path) -> Result<Secret, BuildError> {
    credentials::load_token(dir, "claude")
        .map_err(|_missing| problem("no Claude token saved; sign in on the Agents tab first"))
}

/// The folder with Codex's saved `auth.json`.
fn codex_login(dir: &Path) -> Result<PathBuf, BuildError> {
    let auth_dir = dir.join("codex");
    if auth_dir.join("auth.json").exists() {
        Ok(auth_dir)
    } else {
        Err(problem(
            "no Codex login saved; sign in on the Agents tab first",
        ))
    }
}

/// The `HOME` Antigravity was logged in with.
fn antigravity_login(dir: &Path) -> Result<PathBuf, BuildError> {
    let auth_dir = dir.join("antigravity");
    if auth_dir.join(".gemini/antigravity-cli").is_dir() {
        Ok(auth_dir)
    } else {
        Err(problem(
            "no Antigravity login saved; sign in on the Agents tab first",
        ))
    }
}

/// dsh knows only a few effort words; anything else is refused before it runs.
fn check_dsh_effort(choice: &AgentChoice) -> Result<(), BuildError> {
    match choice.effort {
        Some(effort) if !dsh::EFFORTS.contains(&effort) => Err(problem(format!(
            "{} uses dsh with effort {effort:?}; use one of {}",
            choice.who,
            dsh::EFFORTS.join(", ")
        ))),
        _ => Ok(()),
    }
}

/// The DeepSeek API key: one in the shell wins; otherwise the one the Agents
/// tab («Sign in») saved.
fn deepseek_key(dir: &Path) -> Result<Secret, BuildError> {
    match std::env::var(dsh::KEY_ENV) {
        Ok(key) if !key.trim().is_empty() => Ok(Secret::new(key.trim())),
        _ => credentials::load_token(dir, "deepseek").map_err(|_missing| {
            problem("no DeepSeek API key saved; sign in on the Agents tab first")
        }),
    }
}
