//! Project settings: `.harness/harness.toml`.
//!
//! ```toml
//! max_rounds = 5
//! agent_timeout_minutes = 30
//!
//! [roles.architect]
//! agent = "claude"
//! model = "opus"        # optional: otherwise the agent's default model
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::handoff::Role;
use crate::task::DEFAULT_MAX_ROUNDS;

pub const CONFIG_FILE: &str = "harness.toml";

/// What `harness init` writes into a new project.
pub const DEFAULT_CONFIG: &str = r#"# Settings of the AI harness for this project.

max_rounds = 5
agent_timeout_minutes = 30

# Which agent works as each role: "claude" (Claude Code) or "codex" (Codex CLI).
# Add `model = "..."` to pick a model, for example "opus" for claude.

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleConfig {
    pub agent: String,
    #[serde(default)]
    pub model: Option<String>,
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
