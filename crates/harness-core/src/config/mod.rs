//! Project settings: `.harness/harness.toml`.
//!
//! ```toml
//! max_rounds = 5
//! agent_timeout_minutes = 30
//!
//! [roles.architect]
//! agent = "claude"
//! model = "opus"        # optional: otherwise the agent's default model
//! effort = "high"       # optional: how hard the model thinks (see `crate::models`)
//! skills = ["write-docs"]  # optional: see `crate::skills`
//! ```
//!
//! The folder also holds `edit` (changing `harness.toml` without losing
//! comments), `save` (checking and committing a changed file) and `projects`
//! (the list of Lisa's projects).

mod agent_kind;
pub mod edit;
pub mod independence;
pub mod projects;
pub mod save;

pub use agent_kind::{AgentKind, UnknownAgent};

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::task::handoff::Role;
use crate::task::DEFAULT_MAX_ROUNDS;

/// The name of the settings file inside a project's `.harness` folder.
pub const CONFIG_FILE: &str = "harness.toml";

/// What `harness init` writes into a new project.
pub const DEFAULT_CONFIG: &str = r#"# Settings of the AI harness for this project.

max_rounds = 5
agent_timeout_minutes = 30

# Which agent works as each role: "claude" (Claude Code), "codex" (Codex CLI),
# "antigravity" (Antigravity CLI) or "dsh" (DeepSeek Harness, DeepSeek's own
# agent; the Agents tab («Sign in») saves the API key).
# Add `model = "..."` to pick a model, for example "opus" for claude or
# "deepseek-v4-pro" for dsh (default "deepseek-flash"), and
# `effort = "..."` for how hard it thinks, for example "high". `harness models`
# lists what each agent offers.
#
# MCP servers: describe each once, then list it in the roles that need it.
#   [mcp.context7]
#   command = "npx"
#   args = ["-y", "@upstash/context7-mcp"]
#   env = { CONTEXT7_API_KEY = "secret:context7" }  # `harness secret set context7`
#   [roles.developer]
#   mcp = ["context7"]
#
# Plugins (Claude Code and Codex): a plugin folder is kept in the project,
# by default in .harness/plugins/<name>/, and listed in the roles that need it.
# `agent` says whose plugin it is: "claude" (.claude-plugin/plugin.json) or
# "codex" (.codex-plugin/plugin.json).
#   [plugins.rust-review]
#   agent = "claude"
#   [roles.security]
#   plugins = ["rust-review"]
# `harness plugin add <name>` copies one from a catalog (`harness marketplace
# add owner/repo`) and writes `source` and `commit` here for later updates.
# A plugin with hooks or its own MCP servers is refused unless its settings
# say `allow_hooks = true` or `allow_mcp = true`. Codex plugins with apps
# (ChatGPT connectors) are refused.
#
# Skills are files in .harness/skills/<name>.md that start with
#   ---
#   description: one line about the skill
#   ---
# Each role picks its own: `skills = ["rust-errors"]` are listed in the prompt and
# the agent reads them when needed; `always_skills = ["style"]` go into the prompt
# in full.
#
# `harness retro <task> --suggest` asks the [retro] agent to read the history
# and propose skill changes; nothing changes until `harness retro apply`.

[retro]
agent = "claude"

[roles.architect]
agent = "claude"

[roles.developer]
agent = "claude"

[roles.tester]
agent = "claude"

[roles.security]
agent = "claude"
"#;

/// All settings of one project, as read from `.harness/harness.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `max_rounds`: how many rounds of sent-back work AI roles may do before Lisa decides.
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,
    /// `agent_timeout_minutes`: how long one agent run may take, in minutes, before it is stopped.
    #[serde(default = "default_timeout")]
    pub agent_timeout_minutes: u64,
    /// `BTreeMap` keeps the roles sorted, so saved files are stable.
    #[serde(default)]
    pub roles: BTreeMap<Role, RoleConfig>,
    /// MCP servers the roles may use, by name; see `crate::mcp`.
    #[serde(default)]
    pub mcp: BTreeMap<String, McpConfig>,
    /// Agent plugins kept in the project, by name; see `crate::plugins`.
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginConfig>,
    /// The agent that writes `harness retro --suggest`.
    #[serde(default)]
    pub retro: Option<RetroConfig>,
}

/// `[retro]`: which agent reads the history and proposes skill changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetroConfig {
    /// The agent that does the retro.
    pub agent: AgentKind,
    /// The model to use; `None` means the agent's default model.
    #[serde(default)]
    pub model: Option<String>,
    /// The reasoning effort, such as "high"; the agent's default if not set.
    #[serde(default, deserialize_with = "effort")]
    pub effort: Option<String>,
}

/// `[roles.<role>]`: which agent works as one role, and what it gets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleConfig {
    /// The agent that works as this role.
    pub agent: AgentKind,
    /// The model to use; `None` means the agent's default model.
    #[serde(default)]
    pub model: Option<String>,
    /// The reasoning effort, such as "high"; the agent's default if not set.
    #[serde(default, deserialize_with = "effort")]
    pub effort: Option<String>,
    /// Skills from `.harness/skills/` the agent reads when it needs them.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Skills put into the prompt in full every time.
    #[serde(default)]
    pub always_skills: Vec<String>,
    /// Names of the `[mcp.<name>]` servers this role gets.
    #[serde(default)]
    pub mcp: Vec<String>,
    /// Names of the `[plugins.<name>]` this role gets.
    #[serde(default)]
    pub plugins: Vec<String>,
}

/// One plugin: a folder inside the project, for one kind of agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConfig {
    /// Which agent can load it: Claude Code or Codex.
    pub agent: AgentKind,
    /// The folder, relative to the project; `.harness/plugins/<name>` if not set.
    #[serde(default)]
    pub path: Option<String>,
    /// A plugin's hooks run commands on their own; refused unless allowed here.
    #[serde(default)]
    pub allow_hooks: bool,
    /// A plugin's own MCP or LSP servers; refused unless allowed here.
    #[serde(default)]
    pub allow_mcp: bool,
    /// Where `harness plugin add` took it from: `<catalog>/<plugin>`.
    #[serde(default)]
    pub source: Option<String>,
    /// The git commit it was copied at, so updates can show what changed.
    #[serde(default)]
    pub commit: Option<String>,
}

/// One MCP server: a program the agent starts (`command`, `stdio`), or a
/// server on the web (`url`) the agent reaches through `harness mcp-remote`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpConfig {
    /// The program that is the server; `None` for a server on the web.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Command-line arguments for `command`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Variables for the server. A value `"secret:<name>"` is read from the
    /// secret `harness secret set <name>` saved; the file keeps only the name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The address of a server on the web (streamable HTTP).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP headers for a web server, such as
    /// `Authorization = "Bearer secret:<name>"`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// How Lisa signs in to a web server; `None`: it needs no sign-in, or
    /// its headers carry the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<McpAuth>,
}

/// How Lisa signs in to an MCP server on the web.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpAuth {
    /// `auth = "oauth"`: a sign-in in the browser, once
    /// (`harness mcp login <name>`); the harness keeps the tokens.
    #[serde(rename = "oauth")]
    OAuth,
}

impl McpAuth {
    /// The name as written in `harness.toml`.
    pub fn as_str(self) -> &'static str {
        match self {
            McpAuth::OAuth => "oauth",
        }
    }
}

/// Why `harness.toml` could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read, for example because it is missing.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The file that could not be read, ready to show.
        path: String,
        /// The error from the operating system.
        #[source]
        source: std::io::Error,
    },
    /// The file was read but its contents are not valid settings.
    #[error("{path} is not valid: {source}")]
    Toml {
        /// The file that is not valid, ready to show.
        path: String,
        /// What is wrong, with the line, from the TOML reader.
        #[source]
        source: toml::de::Error,
    },
    /// The file has no `[roles.<role>]` for an AI role that needs one.
    #[error("no agent is set for the {0:?} role in harness.toml")]
    MissingRole(Role),
}

/// An effort level is a plain word: it goes into an agent's command line.
fn effort<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(d)?;
    match &value {
        Some(v) if v.is_empty() || !v.chars().all(|c| c.is_ascii_lowercase()) => {
            Err(serde::de::Error::custom(format!(
                "effort {v:?} is not allowed; use a word such as \"high\""
            )))
        }
        _ => Ok(value),
    }
}

fn default_max_rounds() -> u32 {
    DEFAULT_MAX_ROUNDS
}

fn default_timeout() -> u64 {
    30
}

impl Config {
    /// Reads settings from the text of a `harness.toml` file.
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Reads `<harness_dir>/harness.toml`.
    pub fn load(harness_dir: &Path) -> Result<Self, ConfigError> {
        let path = harness_dir.join(CONFIG_FILE);
        let shown = path.display().to_string();
        let text = fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: shown.clone(),
            source,
        })?;
        Self::parse(&text).map_err(|source| ConfigError::Toml {
            path: shown,
            source,
        })
    }

    /// Every agent some role uses, each once.
    pub fn agents(&self) -> Vec<AgentKind> {
        let mut agents: Vec<AgentKind> = self.roles.values().map(|role| role.agent).collect();
        agents.sort();
        agents.dedup();
        agents
    }

    /// The settings of one AI role; every AI role must have them.
    pub fn role(&self, role: Role) -> Result<&RoleConfig, ConfigError> {
        self.roles.get(&role).ok_or(ConfigError::MissingRole(role))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_agent_of_the_roles_is_listed_once() {
        let config = Config::parse(
            "[roles.architect]\nagent = \"codex\"\n[roles.developer]\nagent = \"claude\"\n\
             [roles.tester]\nagent = \"codex\"\n",
        )
        .unwrap();

        assert_eq!(config.agents(), [AgentKind::Claude, AgentKind::Codex]);
    }

    #[test]
    fn the_default_config_is_valid() {
        let config = Config::parse(DEFAULT_CONFIG).unwrap();
        assert_eq!(config.max_rounds, 5);
        assert_eq!(config.agent_timeout_minutes, 30);
        for role in [
            Role::Architect,
            Role::Developer,
            Role::Tester,
            Role::Security,
        ] {
            assert_eq!(config.role(role).unwrap().agent, AgentKind::Claude);
        }
        assert_eq!(config.retro.unwrap().agent, AgentKind::Claude);
        assert_eq!(Config::parse("").unwrap().retro, None);
    }

    #[test]
    fn effort_is_a_plain_word() {
        let base = "[roles.tester]\nagent = \"claude\"\n";
        let config = Config::parse(&format!("{base}effort = \"high\"\n")).unwrap();
        assert_eq!(
            config.role(Role::Tester).unwrap().effort.as_deref(),
            Some("high")
        );
        Config::parse(&format!("{base}effort = \"high\\\" x\"\n")).unwrap_err();
        Config::parse(&format!("{base}effort = \"\"\n")).unwrap_err();
    }

    #[test]
    fn model_is_optional() {
        let config = Config::parse(
            "[roles.tester]\nagent = \"claude\"\nmodel = \"sonnet\"\n\
             [roles.developer]\nagent = \"claude\"\n",
        )
        .unwrap();
        assert_eq!(
            config.role(Role::Tester).unwrap().model.as_deref(),
            Some("sonnet")
        );
        assert_eq!(config.role(Role::Developer).unwrap().model, None);
        assert!(matches!(
            config.role(Role::Security),
            Err(ConfigError::MissingRole(Role::Security))
        ));
    }

    #[test]
    fn typos_are_errors() {
        Config::parse("max_round = 5").unwrap_err();
        Config::parse("[roles.tester]\nagnet = \"claude\"").unwrap_err();
        Config::parse("[roles.designer]\nagent = \"claude\"").unwrap_err();
    }

    #[test]
    fn an_unknown_agent_is_refused_with_the_names_to_use() {
        let error = Config::parse("[roles.tester]\nagent = \"gemini\"").unwrap_err();
        assert!(error.to_string().contains("\"antigravity\""), "{error}");
        let error = Config::parse("[roles.tester]\nagent = \"codex+deepseek\"").unwrap_err();
        assert!(error.to_string().contains("use \"dsh\""), "{error}");
    }
}
