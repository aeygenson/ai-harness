//! The agents the harness can run a role on, as one enum instead of strings.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// An agent a role can run on. `harness.toml` writes it as a plain name
/// (`agent = "codex"`), so the file format is the same as with strings.
//
// `try_from`/`into` make serde read and write the enum through a `String`,
// so a wrong name gets our own error message (see `UnknownAgent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum AgentKind {
    /// Claude Code.
    Claude,
    /// Codex CLI.
    Codex,
    /// Antigravity CLI (`agy`).
    Antigravity,
    /// `DeepSeek` Harness (`dsh`).
    Dsh,
}

impl AgentKind {
    /// Every agent, in the order the screens list them.
    pub const ALL: [AgentKind; 4] = [
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::Antigravity,
        AgentKind::Dsh,
    ];

    /// The name as written in `harness.toml`, e.g. `"codex"`.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Antigravity => "antigravity",
            AgentKind::Dsh => "dsh",
        }
    }

    /// Whose login the agent needs, the folder name in `~/.harness/credentials/`:
    /// `DeepSeek` Harness uses the `DeepSeek` key.
    pub fn login_name(self) -> &'static str {
        match self {
            AgentKind::Dsh => "deepseek",
            AgentKind::Claude | AgentKind::Codex | AgentKind::Antigravity => self.as_str(),
        }
    }

    /// Can the agent load plugins? Only Claude Code and Codex have them.
    pub fn has_plugins(self) -> bool {
        match self {
            AgentKind::Claude | AgentKind::Codex => true,
            AgentKind::Antigravity | AgentKind::Dsh => false,
        }
    }
}

/// `{agent}` in `format!` prints the same name as [`AgentKind::as_str`].
impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad` (not `write_str`) keeps widths like `{agent:<12}` working.
        f.pad(self.as_str())
    }
}

/// The error when a text is not one of the agent names.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnknownAgent {
    /// The old DeepSeek-inside-Codex agent, replaced by `DeepSeek` Harness.
    #[error(
        "agent \"codex+deepseek\" was removed; use \"dsh\" (DeepSeek Harness, \
         with the same DeepSeek key)"
    )]
    Removed,
    /// Any other text that is not an agent name.
    #[error("{0:?} is not an agent; use \"claude\", \"codex\", \"antigravity\" or \"dsh\"")]
    Other(String),
}

/// Lets `"codex".parse::<AgentKind>()` turn a name back into an agent.
impl FromStr for AgentKind {
    type Err = UnknownAgent;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "claude" => Ok(AgentKind::Claude),
            "codex" => Ok(AgentKind::Codex),
            "antigravity" => Ok(AgentKind::Antigravity),
            "dsh" => Ok(AgentKind::Dsh),
            "codex+deepseek" => Err(UnknownAgent::Removed),
            _ => Err(UnknownAgent::Other(text.to_string())),
        }
    }
}

/// Used by serde to read the name from `harness.toml`.
impl TryFrom<String> for AgentKind {
    type Error = UnknownAgent;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

/// Used by serde to write the name into `harness.toml`.
impl From<AgentKind> for String {
    fn from(agent: AgentKind) -> Self {
        agent.as_str().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_is_read_back_from_its_name() {
        for agent in AgentKind::ALL {
            assert_eq!(agent.as_str().parse::<AgentKind>(), Ok(agent));
            assert_eq!(agent.to_string(), agent.as_str());
        }
    }

    #[test]
    fn a_wrong_name_says_what_to_use() {
        let error = "gemini".parse::<AgentKind>().unwrap_err();
        assert!(error.to_string().contains("\"dsh\""));
        let removed = "codex+deepseek".parse::<AgentKind>().unwrap_err();
        assert_eq!(removed, UnknownAgent::Removed);
        assert!(removed.to_string().contains("use \"dsh\""));
    }

    #[test]
    fn deepseek_harness_uses_the_deepseek_login() {
        assert_eq!(AgentKind::Dsh.login_name(), "deepseek");
        assert_eq!(AgentKind::Codex.login_name(), "codex");
    }

    #[test]
    fn only_claude_and_codex_have_plugins() {
        let with: Vec<AgentKind> = AgentKind::ALL
            .into_iter()
            .filter(|a| a.has_plugins())
            .collect();
        assert_eq!(with, [AgentKind::Claude, AgentKind::Codex]);
    }
}
