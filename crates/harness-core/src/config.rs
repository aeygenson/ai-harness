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

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::handoff::Role;
use crate::task::DEFAULT_MAX_ROUNDS;

pub const CONFIG_FILE: &str = "harness.toml";

/// The agents a role can run on.
pub const AGENTS: [&str; 4] = ["claude", "codex", "codex+deepseek", "antigravity"];

/// What `harness init` writes into a new project.
pub const DEFAULT_CONFIG: &str = r#"# Settings of the AI harness for this project.

max_rounds = 5
agent_timeout_minutes = 30

# Which agent works as each role: "claude" (Claude Code), "codex" (Codex CLI),
# "codex+deepseek" (Codex CLI with DeepSeek models; `harness login deepseek`
# saves the API key) or "antigravity" (Antigravity CLI).
# Add `model = "..."` to pick a model, for example "opus" for claude or
# "deepseek-v4-pro" for codex+deepseek (default "deepseek-flash"), and
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
# "codex" (.codex-plugin/plugin.json, also for "codex+deepseek" roles).
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,
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
    pub agent: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, deserialize_with = "effort")]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleConfig {
    pub agent: String,
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
    /// Which agent can load it: "claude" or "codex".
    pub agent: String,
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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
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
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid: {source}")]
    Toml {
        path: String,
        #[source]
        source: toml::de::Error,
    },
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

    /// The settings of one AI role; every AI role must have them.
    pub fn role(&self, role: Role) -> Result<&RoleConfig, ConfigError> {
        self.roles.get(&role).ok_or(ConfigError::MissingRole(role))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert_eq!(config.role(role).unwrap().agent, "claude");
        }
        assert_eq!(config.retro.unwrap().agent, "claude");
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
        assert!(Config::parse(&format!("{base}effort = \"high\\\" x\"\n")).is_err());
        assert!(Config::parse(&format!("{base}effort = \"\"\n")).is_err());
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
        assert!(Config::parse("max_round = 5").is_err());
        assert!(Config::parse("[roles.tester]\nagnet = \"claude\"").is_err());
        assert!(Config::parse("[roles.designer]\nagent = \"claude\"").is_err());
    }
}
